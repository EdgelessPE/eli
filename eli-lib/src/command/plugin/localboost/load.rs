use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
#[cfg(windows)]
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::fs;
use std::io;
use std::path::Path;
#[cfg(any(windows, test))]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(windows, test))]
use super::repository;
#[cfg(windows)]
use super::repository::SelectionMode;
#[cfg(windows)]
use super::runtime::{LocalBoostLock, RuntimePaths, commit_loaded, is_loaded, safe_component};

#[cfg(any(windows, test))]
use super::super::load::plugin_name;
#[cfg(windows)]
use super::super::load::{
    FileSnapshot, MergeTransaction, ProcessPublishLock, copy_file_replacing, is_reparse_point,
    next_counter, optional_symlink_metadata, reject_reparse_points, replace_file, run_checked,
    with_merge_transaction, write_manifest,
};

#[cfg(windows)]
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Loaded,
    LoadedWithCompatibilityWarning,
    AlreadyLoaded,
}

/// 将一个 LocalBoost 插件包安装到已选择的仓库并加载到当前环境。
pub fn load(ctx: &Ctx, source: &Path) -> io::Result<LoadStatus> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    load_on_supported_platform(ctx, source)
}

#[cfg(windows)]
fn load_on_supported_platform(ctx: &Ctx, source: &Path) -> io::Result<LoadStatus> {
    let programs = ctx.dependencies().require_programs(&[
        ProgramDependency::SevenZip,
        ProgramDependency::Cmd,
        ProgramDependency::Pecmd,
    ])?;
    load_resolved(
        source,
        programs.executable(ProgramDependency::SevenZip)?,
        programs.executable(ProgramDependency::Cmd)?,
        programs.executable(ProgramDependency::Pecmd)?,
    )
}

#[cfg(not(windows))]
fn load_on_supported_platform(_ctx: &Ctx, _source: &Path) -> io::Result<LoadStatus> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "LocalBoost loading is only implemented for Windows PE",
    ))
}

#[cfg(windows)]
pub(crate) fn load_resolved(
    source: &Path,
    seven_zip: &Path,
    cmd: &Path,
    pecmd: &Path,
) -> io::Result<LoadStatus> {
    if !fs::metadata(source).is_ok_and(|metadata| metadata.is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("LocalBoost package is not a file: {}", source.display()),
        ));
    }
    let name = plugin_name(source)?;
    if !safe_component(&name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "LocalBoost package has an unsafe plugin name: {}",
                source.display()
            ),
        ));
    }
    let paths = RuntimePaths::detect()?;
    let repository = repository::select(&paths, SelectionMode::CreateIfMissing)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no LocalBoost repository"))?;
    let lifecycle_lock = LocalBoostLock::new()?;
    let _lifecycle_guard = lifecycle_lock.acquire()?;
    if is_loaded(&paths, &name) {
        return Ok(LoadStatus::AlreadyLoaded);
    }
    reject_existing_reparse_ancestors(&repository)?;
    fs::create_dir_all(&repository)?;
    reject_directory_reparse_point(&repository)?;
    let staging_root = repository
        .parent()
        .ok_or_else(|| io::Error::other("LocalBoost repository has no parent directory"))?
        .join(".eli-staging");
    reject_existing_reparse_ancestors(&staging_root)?;
    fs::create_dir_all(&staging_root)?;
    reject_directory_reparse_point(&staging_root)?;
    let staging = unique_staging_path(&staging_root, &name);
    fs::create_dir(&staging)?;

    let result = (|| {
        let mut output_argument = OsString::from("-o");
        output_argument.push(&staging);
        run_checked(
            Command::new(seven_zip)
                .arg("x")
                .arg(source)
                .arg("-y")
                .arg("-aos")
                .arg(output_argument),
            &format!("extract LocalBoost package {}", source.display()),
        )?;
        reject_reparse_points(&staging)?;

        let unit = repository.join(&name);
        add_local_boost_markers(&staging, &unit)?;
        let unit_guard = CreatedDirectoryGuard::new(&unit)?;
        reject_directory_reparse_point(&unit)?;
        let status = with_merge_transaction(&staging_root, |repository_transaction| {
            merge_directory(&staging, &unit, repository_transaction)?;
            reject_reparse_points(&unit)?;
            restore_unit(&name, &unit, &paths, cmd, pecmd)
        })?;
        unit_guard.commit();
        Ok(status)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(windows)]
pub(crate) fn prepare_repository() -> io::Result<()> {
    let paths = RuntimePaths::detect()?;
    repository::select(&paths, SelectionMode::CreateIfMissing)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no LocalBoost repository"))?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn reject_directory_reparse_point(path: &Path) -> io::Result<()> {
    if optional_symlink_metadata(path)?
        .as_ref()
        .is_some_and(is_reparse_point)
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("LocalBoost path is a reparse point: {}", path.display()),
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn reject_existing_reparse_ancestors(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        if optional_symlink_metadata(ancestor)?
            .as_ref()
            .is_some_and(is_reparse_point)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "LocalBoost path has a reparse-point ancestor: {}",
                    ancestor.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn unique_staging_path(parent: &Path, plugin_name: &OsStr) -> PathBuf {
    parent.join(format!(
        ".eli-{}-{}-{}",
        plugin_name.to_string_lossy(),
        std::process::id(),
        STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(windows)]
fn merge_directory(
    source: &Path,
    destination: &Path,
    transaction: &mut MergeTransaction,
) -> io::Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        transaction.merge(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn add_local_boost_markers(staging: &Path, unit: &Path) -> io::Result<()> {
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let final_directory = unit.join(entry.file_name());
            fs::write(
                entry.path().join("_LocalBoost.txt"),
                final_directory.as_os_str().to_string_lossy().as_bytes(),
            )?;
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn restore_unit(
    plugin_name: &OsStr,
    unit: &Path,
    paths: &RuntimePaths,
    cmd: &Path,
    pecmd: &Path,
) -> io::Result<LoadStatus> {
    if is_loaded(paths, plugin_name) {
        return Ok(LoadStatus::AlreadyLoaded);
    }
    if !safe_unit_directory(unit)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "LocalBoost unit is not a safe directory: {}",
                unit.display()
            ),
        ));
    }
    reject_reparse_points(unit)?;
    let compatibility_warning = contains_nested_batch_script(unit)?;
    // 锁顺序固定为 LocalBoost 生命周期锁在外、普通插件发布锁在内。
    let publish_lock = ProcessPublishLock::new()?;
    let _publish_guard = publish_lock.acquire()?;
    publish_and_run(plugin_name, unit, paths, cmd, pecmd)?;
    Ok(if compatibility_warning {
        LoadStatus::LoadedWithCompatibilityWarning
    } else {
        LoadStatus::Loaded
    })
}

#[cfg(windows)]
fn safe_unit_directory(unit: &Path) -> io::Result<bool> {
    Ok(fs::metadata(unit).is_ok_and(|metadata| metadata.is_dir())
        && unit.file_name().is_some_and(super::runtime::safe_component))
}

#[cfg(windows)]
#[derive(Debug)]
struct CreatedDirectoryGuard {
    path: PathBuf,
    created: bool,
}

#[cfg(windows)]
impl CreatedDirectoryGuard {
    fn new(path: &Path) -> io::Result<Self> {
        let created = optional_symlink_metadata(path)?.is_none();
        fs::create_dir_all(path)?;
        Ok(Self {
            path: path.to_owned(),
            created,
        })
    }

    fn commit(mut self) {
        self.created = false;
    }
}

#[cfg(windows)]
impl Drop for CreatedDirectoryGuard {
    fn drop(&mut self) {
        if self.created {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct Inventory {
    cmd: Vec<PathBuf>,
    wcs: Vec<PathBuf>,
    files: Vec<OsString>,
    directories: Vec<OsString>,
}

#[cfg(windows)]
fn inventory(unit: &Path) -> io::Result<Inventory> {
    let mut inventory = Inventory {
        cmd: Vec::new(),
        wcs: Vec::new(),
        files: Vec::new(),
        directories: Vec::new(),
    };
    let mut entries = fs::read_dir(unit)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            inventory.directories.push(entry.file_name());
        } else if file_type.is_file() {
            if extension_is(&entry.path(), "cmd") {
                inventory.cmd.push(entry.path());
            } else if extension_is(&entry.path(), "wcs") {
                inventory.wcs.push(entry.path());
            } else {
                inventory.files.push(entry.file_name());
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported LocalBoost entry: {}", entry.path().display()),
            ));
        }
    }
    Ok(inventory)
}

#[cfg(windows)]
fn publish_and_run(
    plugin_name: &OsStr,
    unit: &Path,
    paths: &RuntimePaths,
    cmd: &Path,
    pecmd: &Path,
) -> io::Result<()> {
    fs::create_dir_all(&paths.edgeless)?;
    fs::create_dir_all(&paths.installers)?;
    for directory in ["Batch", "Dir", "File"] {
        fs::create_dir_all(paths.plugin_info.join(directory))?;
    }
    fs::create_dir_all(&paths.local_boost)?;
    let inventory = inventory(unit)?;
    let counter_path = paths.local_boost.join("Counter.txt");
    let metadata_paths = [
        counter_path.clone(),
        manifest_path(&paths.plugin_info.join("Batch"), plugin_name),
        manifest_path(&paths.plugin_info.join("File"), plugin_name),
        manifest_path(&paths.plugin_info.join("Dir"), plugin_name),
        paths.plugin_info.join("List_LocalBoost.txt"),
    ];
    let snapshots = metadata_paths
        .into_iter()
        .map(FileSnapshot::capture)
        .collect::<io::Result<Vec<_>>>()?;
    let mut junctions = JunctionTransaction::default();
    let result = with_copy_transaction(&paths.local_boost, |files| {
        let counter = next_counter(&counter_path)?;
        for file in &inventory.files {
            files.copy(&unit.join(file), &paths.edgeless.join(file))?;
        }
        for directory in &inventory.directories {
            let source = unit.join(directory);
            if ensure_junction(cmd, &paths.edgeless.join(directory), &source)? {
                junctions.created.push(paths.edgeless.join(directory));
            }
        }

        let scripts = expose_scripts(&inventory, counter, &paths.edgeless, files)?;
        write_manifest(
            &paths.plugin_info.join("Batch"),
            plugin_name,
            &scripts
                .iter()
                .filter_map(|script| script.file_name().map(OsStr::to_owned))
                .collect::<Vec<_>>(),
        )?;
        write_manifest(
            &paths.plugin_info.join("File"),
            plugin_name,
            &inventory.files,
        )?;
        write_manifest(
            &paths.plugin_info.join("Dir"),
            plugin_name,
            &inventory.directories,
        )?;
        update_plugin_list(&paths.plugin_info.join("List_LocalBoost.txt"), plugin_name)?;
        run_and_archive_scripts(
            &scripts,
            &paths.edgeless,
            &paths.installers,
            cmd,
            pecmd,
            files,
        )?;
        commit_loaded(paths, plugin_name)
    });
    match result {
        Ok(()) => {
            junctions.commit();
            Ok(())
        }
        Err(error) => {
            let restore_errors = snapshots
                .iter()
                .filter_map(|snapshot| snapshot.restore().err())
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            if restore_errors.is_empty() {
                Err(error)
            } else {
                Err(io::Error::new(
                    error.kind(),
                    format!(
                        "{error}; failed to restore metadata: {}",
                        restore_errors.join("; ")
                    ),
                ))
            }
        }
    }
}

#[cfg(windows)]
fn contains_nested_batch_script(unit: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(unit)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        for nested in fs::read_dir(entry.path())? {
            let nested = nested?;
            if nested.file_type()?.is_file()
                && (extension_is(&nested.path(), "bat") || extension_is(&nested.path(), "cmd"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn manifest_path(directory: &Path, plugin_name: &OsStr) -> PathBuf {
    let mut path = directory.join(plugin_name).into_os_string();
    path.push(".txt");
    PathBuf::from(path)
}

#[cfg(windows)]
#[derive(Debug)]
struct CopyTransaction {
    backup_root: PathBuf,
    changes: Vec<CopyChange>,
    finished: bool,
}

#[cfg(windows)]
#[derive(Debug)]
enum CopyChange {
    Created(PathBuf),
    Replaced {
        destination: PathBuf,
        backup: PathBuf,
    },
}

#[cfg(windows)]
impl CopyTransaction {
    fn new(parent: &Path) -> io::Result<Self> {
        let backup_root = unique_staging_path(parent, OsStr::new("publish-backup"));
        fs::create_dir(&backup_root)?;
        Ok(Self {
            backup_root,
            changes: Vec::new(),
            finished: false,
        })
    }

    fn copy(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        let destination_metadata = optional_symlink_metadata(destination)?;
        if destination_metadata.as_ref().is_some_and(is_reparse_point) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "LocalBoost destination is a reparse point: {}",
                    destination.display()
                ),
            ));
        }
        if destination_metadata.is_some() {
            let backup = self.backup_root.join(self.changes.len().to_string());
            fs::rename(destination, &backup)?;
            self.changes.push(CopyChange::Replaced {
                destination: destination.to_owned(),
                backup,
            });
        } else {
            self.changes
                .push(CopyChange::Created(destination.to_owned()));
        }
        copy_file_replacing(source, destination)?;
        Ok(())
    }

    fn commit(mut self) {
        self.finished = true;
        let _ = fs::remove_dir_all(&self.backup_root);
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut failures = Vec::new();
        for change in self.changes.iter().rev() {
            match change {
                CopyChange::Created(destination) => {
                    if let Err(error) = fs::remove_file(destination)
                        && error.kind() != io::ErrorKind::NotFound
                    {
                        failures.push(format!("remove {}: {error}", destination.display()));
                    }
                }
                CopyChange::Replaced {
                    destination,
                    backup,
                } => {
                    if let Err(error) = match fs::symlink_metadata(destination) {
                        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(destination),
                        Ok(_) => fs::remove_file(destination),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(error),
                    } {
                        failures.push(format!("remove {}: {error}", destination.display()));
                    }
                    if let Err(error) = fs::rename(backup, destination) {
                        failures.push(format!(
                            "restore {} from {}: {error}",
                            destination.display(),
                            backup.display()
                        ));
                    }
                }
            }
        }
        if failures.is_empty()
            && let Err(error) = fs::remove_dir_all(&self.backup_root)
        {
            failures.push(format!(
                "remove rollback directory {}: {error}",
                self.backup_root.display()
            ));
        }
        self.finished = true;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
    }
}

#[cfg(windows)]
impl Drop for CopyTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}

#[cfg(windows)]
fn with_copy_transaction<T>(
    parent: &Path,
    operation: impl FnOnce(&mut CopyTransaction) -> io::Result<T>,
) -> io::Result<T> {
    let mut transaction = CopyTransaction::new(parent)?;
    match operation(&mut transaction) {
        Ok(value) => {
            transaction.commit();
            Ok(value)
        }
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(io::Error::new(
                error.kind(),
                format!("{error}; LocalBoost rollback also failed: {rollback_error}"),
            )),
        },
    }
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct JunctionTransaction {
    created: Vec<PathBuf>,
    finished: bool,
}

#[cfg(windows)]
impl JunctionTransaction {
    fn commit(mut self) {
        self.finished = true;
    }
}

#[cfg(windows)]
impl Drop for JunctionTransaction {
    fn drop(&mut self) {
        if !self.finished {
            for junction in self.created.iter().rev() {
                let _ = fs::remove_dir(junction);
            }
        }
    }
}

#[cfg(windows)]
fn expose_scripts(
    inventory: &Inventory,
    counter: u64,
    edgeless: &Path,
    transaction: &mut CopyTransaction,
) -> io::Result<Vec<PathBuf>> {
    let mut exposed = Vec::new();
    for (extension, scripts) in [("cmd", &inventory.cmd), ("wcs", &inventory.wcs)] {
        for script in scripts {
            let stem = script.file_stem().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("LocalBoost script has no stem: {}", script.display()),
                )
            })?;
            let destination = edgeless.join(format!(
                "{}_localboost_{counter}.{extension}",
                stem.to_string_lossy()
            ));
            transaction.copy(script, &destination)?;
            exposed.push(destination);
        }
    }
    Ok(exposed)
}

#[cfg(windows)]
fn ensure_junction(cmd: &Path, destination: &Path, source: &Path) -> io::Result<bool> {
    if destination.exists() {
        if fs::canonicalize(destination)? == fs::canonicalize(source)? {
            return Ok(false);
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "LocalBoost junction destination already exists: {}",
                destination.display()
            ),
        ));
    }
    run_checked(
        Command::new(cmd)
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(destination)
            .arg(source),
        &format!("create LocalBoost junction {}", destination.display()),
    )?;
    Ok(true)
}

#[cfg(windows)]
fn run_and_archive_scripts(
    scripts: &[PathBuf],
    working_directory: &Path,
    installers: &Path,
    cmd: &Path,
    pecmd: &Path,
    transaction: &mut CopyTransaction,
) -> io::Result<()> {
    let mut failures = Vec::new();
    for script in scripts.iter().filter(|script| extension_is(script, "cmd")) {
        if let Err(error) = run_checked(
            Command::new(cmd)
                .arg("/d")
                .arg("/c")
                .arg(script)
                .current_dir(working_directory),
            &format!("run LocalBoost CMD script {}", script.display()),
        ) {
            failures.push(error.to_string());
        }
    }
    for script in scripts.iter().filter(|script| extension_is(script, "wcs")) {
        if let Err(error) = run_checked(
            Command::new(pecmd)
                .arg("LOAD")
                .arg(script)
                .current_dir(working_directory),
            &format!("run LocalBoost WCS script {}", script.display()),
        ) {
            failures.push(error.to_string());
        }
    }
    for script in scripts {
        let Some(name) = script.file_name() else {
            failures.push(format!(
                "LocalBoost script has no file name: {}",
                script.display()
            ));
            continue;
        };
        let destination = installers.join(name);
        if let Err(error) = transaction
            .copy(script, &destination)
            .and_then(|()| fs::remove_file(script))
        {
            failures.push(format!(
                "failed to archive LocalBoost script {}: {error}",
                script.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

#[cfg(windows)]
fn update_plugin_list(path: &Path, plugin_name: &OsStr) -> io::Result<()> {
    use super::runtime::{CompatibleText, read_compatible_text};

    let name = plugin_name.to_string_lossy();
    let existing = read_compatible_text(path)?;
    let mut entries = match existing.as_ref() {
        Some(text) => {
            let mut entries = Vec::<String>::new();
            for line in text.contents.lines().filter(|line| !line.is_empty()) {
                if !entries.iter().any(|entry| entry.eq_ignore_ascii_case(line)) {
                    entries.push(line.to_owned());
                }
            }
            entries
        }
        None => Vec::new(),
    };
    if !entries
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(&name))
    {
        entries.push(name.into_owned());
    }
    let newline = existing.as_ref().map_or("\n", CompatibleText::newline);
    let contents = entries.join(newline) + newline;
    let bytes = match existing {
        Some(text) => text.encode(&contents)?,
        None => CompatibleText::decode(b"")?.encode(&contents)?,
    };
    replace_file(path, &bytes)
}

#[cfg(windows)]
fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::env;

    #[cfg(windows)]
    fn test_root() -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = env::temp_dir().join(format!(
            "eli-localboost-load-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn parses_the_persistent_repository_drive_selection() {
        assert_eq!(
            repository::repository_from_selection(" d:\r\n"),
            Some(PathBuf::from(r"D:\Edgeless\BoostRepo"))
        );
        assert_eq!(
            repository::repository_from_selection("\"e\""),
            Some(PathBuf::from(r"E:\Edgeless\BoostRepo"))
        );
    }

    #[test]
    fn rejects_non_drive_repository_selections() {
        assert_eq!(repository::repository_from_selection(""), None);
        assert_eq!(repository::repository_from_selection("D:\\other"), None);
        assert_eq!(repository::repository_from_selection("../D"), None);
    }

    #[test]
    fn accepts_arbitrary_extensions_and_extensionless_names() {
        assert_eq!(plugin_name(Path::new("tool.custom")).unwrap(), "tool");
        assert_eq!(plugin_name(Path::new("tool")).unwrap(), "tool");
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_repository_with_a_reparse_point_ancestor() {
        use std::process::{Command, Stdio};

        let root = test_root();
        let outside = root.join("outside");
        let linked_parent = root.join("linked");
        fs::create_dir_all(&outside).unwrap();
        let status = Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&linked_parent)
            .arg(&outside)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let error =
            reject_existing_reparse_ancestors(&linked_parent.join("BoostRepo")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!outside.join("BoostRepo").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn copy_transaction_restores_replaced_runtime_files() {
        let root = test_root();
        let backup_parent = root.join("backups");
        let source = root.join("new.dll");
        let destination = root.join("runtime.dll");
        fs::create_dir_all(&backup_parent).unwrap();
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();

        {
            let mut transaction = CopyTransaction::new(&backup_parent).unwrap();
            transaction.copy(&source, &destination).unwrap();
            assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
        }

        assert_eq!(fs::read_to_string(&destination).unwrap(), "old");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn activates_root_files_and_writes_compatible_metadata_names() {
        let root = test_root();
        let unit = root.join("repository/tool_1.2_author");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("payload.dll"), "payload").unwrap();
        let paths = RuntimePaths {
            edgeless: root.join("Program Files/Edgeless"),
            installers: root.join("Program Files/Edgeless/安装程序"),
            system_drive: root.clone(),
            plugin_info: root.join("Users/Plugins_info"),
            local_boost: root.join("Users/LocalBoost"),
            selection_file: root.join("unused-repoPart.txt"),
            loaded: root.join("Users/LocalBoost/Loaded"),
        };

        publish_and_run(
            OsStr::new("tool_1.2_author"),
            &unit,
            &paths,
            Path::new("unused-cmd.exe"),
            Path::new("unused-pecmd.exe"),
        )
        .unwrap();
        publish_and_run(
            OsStr::new("tool_1.2_author"),
            &unit,
            &paths,
            Path::new("unused-cmd.exe"),
            Path::new("unused-pecmd.exe"),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(paths.edgeless.join("payload.dll")).unwrap(),
            "payload"
        );
        assert!(paths.plugin_info.join("File/tool_1.2_author.txt").exists());
        assert_eq!(
            fs::read_to_string(paths.plugin_info.join("File/tool_1.2_author.txt")).unwrap(),
            "payload.dll\n"
        );
        assert_eq!(
            fs::read_to_string(paths.plugin_info.join("List_LocalBoost.txt")).unwrap(),
            "tool_1.2_author\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn detects_cmd_and_bat_files_in_first_level_directories() {
        let root = test_root();
        let unit = root.join("unit");
        fs::create_dir_all(unit.join("dependency/nested")).unwrap();

        assert!(!contains_nested_batch_script(&unit).unwrap());
        fs::write(unit.join("dependency/setup.cmd"), "").unwrap();
        assert!(contains_nested_batch_script(&unit).unwrap());
        fs::remove_file(unit.join("dependency/setup.cmd")).unwrap();
        fs::write(unit.join("dependency/setup.BAT"), "").unwrap();
        assert!(contains_nested_batch_script(&unit).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn script_failure_rolls_back_runtime_metadata_and_counter() {
        let root = test_root();
        let unit = root.join("repository/tool");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("payload.dll"), "new").unwrap();
        fs::write(unit.join("fail.cmd"), "@exit /b 7\r\n").unwrap();
        let paths = RuntimePaths {
            edgeless: root.join("Program Files/Edgeless"),
            installers: root.join("Program Files/Edgeless/安装程序"),
            system_drive: root.clone(),
            plugin_info: root.join("Users/Plugins_info"),
            local_boost: root.join("Users/LocalBoost"),
            selection_file: root.join("unused-repoPart.txt"),
            loaded: root.join("Users/LocalBoost/Loaded"),
        };
        fs::create_dir_all(&paths.edgeless).unwrap();
        fs::create_dir_all(&paths.local_boost).unwrap();
        fs::create_dir_all(&paths.plugin_info).unwrap();
        fs::write(paths.edgeless.join("payload.dll"), "old").unwrap();
        fs::write(paths.local_boost.join("Counter.txt"), "7").unwrap();
        fs::write(paths.plugin_info.join("List_LocalBoost.txt"), "old\r\n").unwrap();

        let error = restore_unit(
            OsStr::new("tool"),
            &unit,
            &paths,
            Path::new("cmd.exe"),
            Path::new("unused-pecmd.exe"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("process exited"));
        assert_eq!(
            fs::read_to_string(paths.edgeless.join("payload.dll")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(paths.local_boost.join("Counter.txt")).unwrap(),
            "7"
        );
        assert_eq!(
            fs::read_to_string(paths.plugin_info.join("List_LocalBoost.txt")).unwrap(),
            "old\r\n"
        );
        assert!(!paths.plugin_info.join("Batch/tool.txt").exists());
        assert!(!paths.installers.join("fail_localboost_8.cmd").exists());
        assert!(!is_loaded(&paths, OsStr::new("tool")));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn already_loaded_unit_does_not_publish_or_run_scripts_again() {
        let root = test_root();
        let unit = root.join("repository/tool");
        fs::create_dir_all(&unit).unwrap();
        let paths = RuntimePaths {
            edgeless: root.join("Program Files/Edgeless"),
            installers: root.join("Program Files/Edgeless/安装程序"),
            system_drive: root.clone(),
            plugin_info: root.join("Users/Plugins_info"),
            local_boost: root.join("Users/LocalBoost"),
            selection_file: root.join("unused-repoPart.txt"),
            loaded: root.join("Users/LocalBoost/Loaded"),
        };
        commit_loaded(&paths, OsStr::new("tool")).unwrap();

        let status = restore_unit(
            OsStr::new("tool"),
            &unit,
            &paths,
            Path::new("missing-cmd.exe"),
            Path::new("missing-pecmd.exe"),
        )
        .unwrap();

        assert_eq!(status, LoadStatus::AlreadyLoaded);
        assert!(!paths.edgeless.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
