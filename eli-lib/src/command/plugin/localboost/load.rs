use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
#[cfg(windows)]
use std::env;
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

#[cfg(windows)]
use super::super::load::{
    FileSnapshot, MergeTransaction, ProcessPublishLock, copy_file_replacing, is_reparse_point,
    merge_move, next_counter, optional_symlink_metadata, plugin_name, reject_reparse_points,
    replace_file, run_checked, with_merge_transaction, write_manifest,
};

#[cfg(windows)]
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 将一个 LocalBoost 插件包安装到已选择的仓库并加载到当前环境。
pub fn load(ctx: &Ctx, source: &Path) -> io::Result<()> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    load_on_supported_platform(ctx, source)
}

#[cfg(windows)]
fn load_on_supported_platform(ctx: &Ctx, source: &Path) -> io::Result<()> {
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
fn load_on_supported_platform(_ctx: &Ctx, _source: &Path) -> io::Result<()> {
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
) -> io::Result<()> {
    if !extension_is(source, "7zl") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "LocalBoost package must use the .7zl extension: {}",
                source.display()
            ),
        ));
    }
    if !fs::metadata(source).is_ok_and(|metadata| metadata.is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("LocalBoost package is not a file: {}", source.display()),
        ));
    }
    let name = plugin_name(source)?;
    let paths = RuntimePaths::detect()?;
    let repository = selected_repository(&paths.selection_file, &paths.system_drive)?;
    reject_existing_reparse_ancestors(&repository)?;
    fs::create_dir_all(&repository)?;
    reject_directory_reparse_point(&repository)?;
    let staging_root = repository
        .parent()
        .ok_or_else(|| io::Error::other("LocalBoost repository has no parent directory"))?
        .join(".eli-plugin-release");
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
        let scripts = {
            let lock = ProcessPublishLock::new()?;
            let _guard = lock.acquire()?;
            let unit_guard = CreatedDirectoryGuard::new(&unit)?;
            reject_directory_reparse_point(&unit)?;
            let scripts = with_merge_transaction(&staging_root, |repository_transaction| {
                merge_directory(&staging, &unit, repository_transaction)?;
                reject_reparse_points(&unit)?;
                publish(&name, &unit, &paths, cmd)
            })?;
            unit_guard.commit();
            scripts
        };
        run_and_archive_scripts(&scripts, &paths.edgeless, &paths.installers, cmd, pecmd)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(windows)]
fn reject_directory_reparse_point(path: &Path) -> io::Result<()> {
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
fn reject_existing_reparse_ancestors(path: &Path) -> io::Result<()> {
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
#[derive(Debug)]
struct RuntimePaths {
    edgeless: PathBuf,
    installers: PathBuf,
    system_drive: PathBuf,
    plugin_info: PathBuf,
    local_boost: PathBuf,
    selection_file: PathBuf,
}

#[cfg(windows)]
impl RuntimePaths {
    fn detect() -> io::Result<Self> {
        let program_files = env::var_os("ProgramFiles")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ProgramFiles is not set"))?;
        let system_drive = env::var_os("SystemDrive")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemDrive is not set"))?;
        let system_drive = PathBuf::from(system_drive);
        let users = system_drive.join("Users");
        let local_boost = users.join("LocalBoost");
        let edgeless = PathBuf::from(program_files).join("Edgeless");
        Ok(Self {
            installers: edgeless.join("安装程序"),
            edgeless,
            system_drive,
            plugin_info: users.join("Plugins_info"),
            selection_file: local_boost.join("repoPart.txt"),
            local_boost,
        })
    }
}

#[cfg(windows)]
fn selected_repository(selection_file: &Path, system_drive: &Path) -> io::Result<PathBuf> {
    let selection = fs::read_to_string(selection_file).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read the LocalBoost repository selection {}: {error}",
                selection_file.display()
            ),
        )
    })?;
    let repository = repository_from_selection(&selection).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid LocalBoost repository selection in {}",
                selection_file.display()
            ),
        )
    })?;
    if matches!(
        (drive_letter(&repository), drive_letter(system_drive)),
        (Some(repository_drive), Some(system_drive)) if repository_drive == system_drive
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "LocalBoost repository cannot use the Windows PE system drive",
        ));
    }
    Ok(repository)
}

#[cfg(any(windows, test))]
fn repository_from_selection(selection: &str) -> Option<PathBuf> {
    let selection = selection.trim().trim_matches('"');
    let mut characters = selection.chars();
    let letter = characters.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let remainder = characters.as_str();
    if !remainder.is_empty() && remainder != ":" {
        return None;
    }
    Some(PathBuf::from(format!(
        "{}:\\Edgeless\\BoostRepo",
        letter.to_ascii_uppercase()
    )))
}

#[cfg(windows)]
fn drive_letter(path: &Path) -> Option<char> {
    let value = path.as_os_str().to_string_lossy();
    let mut characters = value.chars();
    let letter = characters.next()?;
    (letter.is_ascii_alphabetic() && characters.next() == Some(':'))
        .then(|| letter.to_ascii_uppercase())
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
fn publish(
    plugin_name: &OsStr,
    unit: &Path,
    paths: &RuntimePaths,
    cmd: &Path,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(&paths.edgeless)?;
    fs::create_dir_all(&paths.installers)?;
    for directory in ["Batch", "Dir", "File"] {
        fs::create_dir_all(paths.plugin_info.join(directory))?;
    }
    fs::create_dir_all(&paths.local_boost)?;
    let inventory = inventory(unit)?;
    let counter = next_counter(&paths.local_boost.join("Counter.txt"))?;
    let mut junctions = JunctionTransaction::default();
    let scripts = with_copy_transaction(&paths.local_boost, |files| {
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
        let metadata_paths = [
            manifest_path(&paths.plugin_info.join("Batch"), plugin_name),
            manifest_path(&paths.plugin_info.join("File"), plugin_name),
            manifest_path(&paths.plugin_info.join("Dir"), plugin_name),
            paths.plugin_info.join("List_LocalBoost.txt"),
        ];
        let snapshots = metadata_paths
            .into_iter()
            .map(FileSnapshot::capture)
            .collect::<io::Result<Vec<_>>>()?;
        let metadata_result = (|| {
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
            update_plugin_list(&paths.plugin_info.join("List_LocalBoost.txt"), plugin_name)
        })();
        if let Err(error) = metadata_result {
            let restore_errors = snapshots
                .iter()
                .filter_map(|snapshot| snapshot.restore().err())
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            if restore_errors.is_empty() {
                return Err(error);
            }
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "{error}; failed to restore metadata: {}",
                    restore_errors.join("; ")
                ),
            ));
        }
        Ok(scripts)
    })?;
    junctions.commit();
    Ok(scripts)
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
        if let Err(error) = merge_move(script, &installers.join(name)) {
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
    let name = plugin_name.to_string_lossy();
    let mut entries = match fs::read_to_string(path) {
        Ok(contents) => {
            let mut entries = Vec::<String>::new();
            for line in contents.lines().filter(|line| !line.is_empty()) {
                if !entries.iter().any(|entry| entry.eq_ignore_ascii_case(line)) {
                    entries.push(line.to_owned());
                }
            }
            entries
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    if !entries
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(&name))
    {
        entries.push(name.into_owned());
    }
    replace_file(path, (entries.join("\n") + "\n").as_bytes())
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
            repository_from_selection(" d:\r\n"),
            Some(PathBuf::from(r"D:\Edgeless\BoostRepo"))
        );
        assert_eq!(
            repository_from_selection("\"e\""),
            Some(PathBuf::from(r"E:\Edgeless\BoostRepo"))
        );
    }

    #[test]
    fn rejects_non_drive_repository_selections() {
        assert_eq!(repository_from_selection(""), None);
        assert_eq!(repository_from_selection("D:\\other"), None);
        assert_eq!(repository_from_selection("../D"), None);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_the_windows_pe_system_drive_as_repository() {
        let root = test_root();
        let selection = root.join("repoPart.txt");
        fs::write(&selection, "X").unwrap();

        let error = selected_repository(&selection, Path::new(r"X:")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        fs::remove_dir_all(root).unwrap();
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
        };

        let scripts = publish(
            OsStr::new("tool_1.2_author"),
            &unit,
            &paths,
            Path::new("unused-cmd.exe"),
        )
        .unwrap();
        let repeated_scripts = publish(
            OsStr::new("tool_1.2_author"),
            &unit,
            &paths,
            Path::new("unused-cmd.exe"),
        )
        .unwrap();

        assert!(scripts.is_empty());
        assert!(repeated_scripts.is_empty());
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
}
