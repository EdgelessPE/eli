use crate::Ctx;
use crate::dependency::RuntimeEnvironment;
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use super::repository::{self, SelectionMode};
#[cfg(windows)]
use super::runtime::{LocalBoostLock, RuntimePaths, clear_loaded, safe_component};

#[cfg(windows)]
static DELETE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanTarget {
    Plugin(OsString),
    All,
}

#[derive(Debug)]
pub struct CleanResult {
    pub repository: PathBuf,
    pub plugin: Option<OsString>,
    pub result: Result<(), io::Error>,
    pub runtime_cleanup_incomplete: bool,
}

#[derive(Debug, Default)]
pub struct CleanSummary {
    pub results: Vec<CleanResult>,
}

impl CleanSummary {
    pub fn succeeded(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.result.is_ok())
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.result.is_err())
            .count()
    }

    pub fn requires_restart(&self) -> bool {
        !self.results.is_empty()
    }

    pub fn runtime_cleanup_incomplete(&self) -> bool {
        self.results
            .iter()
            .any(|result| result.runtime_cleanup_incomplete)
    }

    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

/// 清理选定 LocalBoost unit 或显式清空当前电脑上的全部仓库。
pub fn clean(ctx: &Ctx, target: CleanTarget) -> io::Result<CleanSummary> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    clean_on_supported_platform(target)
}

#[cfg(windows)]
fn clean_on_supported_platform(target: CleanTarget) -> io::Result<CleanSummary> {
    let paths = RuntimePaths::detect()?;
    let lifecycle_lock = LocalBoostLock::new()?;
    let _lifecycle_guard = lifecycle_lock.acquire()?;
    match target {
        CleanTarget::Plugin(plugin) => clean_one_selected(&paths, &plugin),
        CleanTarget::All => clean_all(&paths),
    }
}

#[cfg(not(windows))]
fn clean_on_supported_platform(_target: CleanTarget) -> io::Result<CleanSummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "LocalBoost cleanup is only implemented for Windows PE",
    ))
}

#[cfg(windows)]
fn clean_one_selected(paths: &RuntimePaths, plugin: &OsStr) -> io::Result<CleanSummary> {
    validate_plugin_name(plugin)?;
    let repository = repository::select(paths, SelectionMode::ExistingOnly)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no LocalBoost repository exists")
    })?;
    let result = clean_unit(paths, &repository, plugin);
    Ok(CleanSummary {
        results: vec![result],
    })
}

#[cfg(windows)]
fn clean_all(paths: &RuntimePaths) -> io::Result<CleanSummary> {
    let repositories = repository::all_repositories(paths)?;
    let mut results = Vec::new();
    for repository in repositories {
        let mut units = match safe_units(&repository) {
            Ok(units) => units,
            Err(error) => {
                results.push(CleanResult {
                    repository: repository.clone(),
                    plugin: None,
                    result: Err(error),
                    runtime_cleanup_incomplete: true,
                });
                continue;
            }
        };
        units.sort_unstable_by_key(|name| name.to_string_lossy().to_lowercase());
        for plugin in units {
            results.push(clean_unit(paths, &repository, &plugin));
        }
        let staging = repository
            .parent()
            .expect("BoostRepo has an Edgeless parent")
            .join(".eli-staging");
        if let Err(error) = remove_tree_safely(&staging) {
            results.push(CleanResult {
                repository: repository.clone(),
                plugin: None,
                result: Err(error),
                runtime_cleanup_incomplete: false,
            });
        }
        if repository.is_dir()
            && fs::read_dir(&repository).is_ok_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(&repository);
        }
    }
    if repository::all_repositories(paths)?.is_empty() {
        remove_file_if_exists(&paths.selection_file)?;
    }
    Ok(CleanSummary { results })
}

#[cfg(windows)]
fn safe_units(repository: &Path) -> io::Result<Vec<OsString>> {
    super::load::reject_existing_reparse_ancestors(repository)?;
    super::load::reject_directory_reparse_point(repository)?;
    fs::read_dir(repository)?
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && safe_component(&entry.file_name()) =>
            {
                Some(Ok(entry.file_name()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

#[cfg(windows)]
fn clean_unit(paths: &RuntimePaths, repository: &Path, plugin: &OsStr) -> CleanResult {
    let mut incomplete = false;
    let result = (|| {
        validate_plugin_name(plugin)?;
        let unit = repository.join(plugin);
        if !unit.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "LocalBoost plugin was not found: {}",
                    plugin.to_string_lossy()
                ),
            ));
        }
        super::load::reject_existing_reparse_ancestors(&unit)?;
        super::super::load::reject_reparse_points(&unit)?;

        if cleanup_runtime(paths, &unit, plugin).is_err() {
            incomplete = true;
        }

        let staging_root = repository
            .parent()
            .expect("BoostRepo has an Edgeless parent")
            .join(".eli-staging");
        fs::create_dir_all(&staging_root)?;
        super::load::reject_existing_reparse_ancestors(&staging_root)?;
        super::load::reject_directory_reparse_point(&staging_root)?;
        let deleted = staging_root.join(format!(
            ".eli-delete-{}-{}-{}",
            plugin.to_string_lossy(),
            std::process::id(),
            DELETE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::rename(&unit, &deleted)?;
        if let Err(error) = remove_metadata(paths, plugin) {
            let rollback = fs::rename(&deleted, &unit);
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(io::Error::new(
                    error.kind(),
                    format!("{error}; failed to restore unit: {rollback_error}"),
                )),
            };
        }
        if clear_loaded(paths, plugin).is_err() {
            incomplete = true;
        }
        remove_tree_safely(&deleted)
    })();
    CleanResult {
        repository: repository.to_owned(),
        plugin: Some(plugin.to_owned()),
        result,
        runtime_cleanup_incomplete: incomplete,
    }
}

#[cfg(windows)]
fn cleanup_runtime(paths: &RuntimePaths, unit: &Path, plugin: &OsStr) -> io::Result<()> {
    let mut failures = Vec::new();
    for name in read_manifest(&manifest_path(&paths.plugin_info.join("File"), plugin))? {
        if !safe_component(&name) {
            failures.push(format!(
                "unsafe file manifest entry: {}",
                name.to_string_lossy()
            ));
            continue;
        }
        let source = unit.join(&name);
        let destination = paths.edgeless.join(&name);
        if files_equal(&source, &destination)
            && let Err(error) = remove_file_if_exists(&destination)
        {
            failures.push(error.to_string());
        }
    }
    for name in read_manifest(&manifest_path(&paths.plugin_info.join("Dir"), plugin))? {
        if !safe_component(&name) {
            failures.push(format!(
                "unsafe directory manifest entry: {}",
                name.to_string_lossy()
            ));
            continue;
        }
        let source = unit.join(&name);
        let destination = paths.edgeless.join(&name);
        let belongs = match (fs::canonicalize(&destination), fs::canonicalize(&source)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        };
        if belongs
            && let Err(error) = fs::remove_dir(&destination)
            && error.kind() != io::ErrorKind::NotFound
        {
            failures.push(error.to_string());
        }
    }
    for name in read_manifest(&manifest_path(&paths.plugin_info.join("Batch"), plugin))? {
        if !safe_component(&name) {
            failures.push(format!(
                "unsafe script manifest entry: {}",
                name.to_string_lossy()
            ));
            continue;
        }
        for directory in [&paths.edgeless, &paths.installers] {
            if let Err(error) = remove_file_if_exists(&directory.join(&name)) {
                failures.push(error.to_string());
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

#[cfg(windows)]
fn remove_metadata(paths: &RuntimePaths, plugin: &OsStr) -> io::Result<()> {
    use super::super::load::FileSnapshot;

    let manifests =
        ["Batch", "File", "Dir"].map(|kind| manifest_path(&paths.plugin_info.join(kind), plugin));
    let list = paths.plugin_info.join("List_LocalBoost.txt");
    let mut snapshots = manifests
        .iter()
        .cloned()
        .chain([list.clone()])
        .map(FileSnapshot::capture)
        .collect::<io::Result<Vec<_>>>()?;
    let result = (|| {
        for manifest in &manifests {
            remove_file_if_exists(manifest)?;
        }
        remove_list_entry(&list, plugin)
    })();
    if let Err(error) = result {
        let failures = snapshots
            .drain(..)
            .filter_map(|snapshot| snapshot.restore().err())
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if failures.is_empty() {
            Err(error)
        } else {
            Err(io::Error::new(
                error.kind(),
                format!(
                    "{error}; failed to restore metadata: {}",
                    failures.join("; ")
                ),
            ))
        }
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn remove_list_entry(path: &Path, plugin: &OsStr) -> io::Result<()> {
    use super::super::load::replace_file;
    use super::runtime::read_compatible_text;

    let plugin = plugin.to_string_lossy();
    let Some(text) = read_compatible_text(path)? else {
        return Ok(());
    };
    let entries = text
        .contents
        .lines()
        .filter(|entry| !entry.eq_ignore_ascii_case(&plugin))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        remove_file_if_exists(path)
    } else {
        let newline = text.newline();
        replace_file(path, &text.encode(&(entries.join(newline) + newline))?)
    }
}

#[cfg(windows)]
fn read_manifest(path: &Path) -> io::Result<Vec<OsString>> {
    match super::runtime::read_compatible_text(path)? {
        Some(text) => Ok(text
            .contents
            .lines()
            .filter(|line| !line.is_empty())
            .map(OsString::from)
            .collect()),
        None => Ok(Vec::new()),
    }
}

#[cfg(windows)]
fn manifest_path(directory: &Path, plugin: &OsStr) -> PathBuf {
    let mut path = directory.join(plugin).into_os_string();
    path.push(".txt");
    PathBuf::from(path)
}

#[cfg(windows)]
fn files_equal(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(windows)]
fn remove_tree_safely(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    super::load::reject_existing_reparse_ancestors(path)?;
    super::super::load::reject_reparse_points(path)?;
    fs::remove_dir_all(path)
}

#[cfg(windows)]
fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_plugin_name(plugin: &OsStr) -> io::Result<()> {
    if super::runtime::safe_component(plugin) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "LocalBoost plugin must be a single safe directory name: {}",
                plugin.to_string_lossy()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn test_paths() -> (PathBuf, RuntimePaths) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = std::env::temp_dir().join(format!(
            "eli-localboost-clean-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let edgeless = root.join("Program Files/Edgeless");
        let paths = RuntimePaths {
            installers: edgeless.join("安装程序"),
            edgeless,
            system_drive: root.clone(),
            plugin_info: root.join("Users/Plugins_info"),
            local_boost: root.join("Users/LocalBoost"),
            selection_file: root.join("Users/LocalBoost/repoPart.txt"),
            loaded: root.join("Users/LocalBoost/Loaded"),
        };
        (root, paths)
    }

    #[test]
    fn accepts_a_unicode_plugin_directory_name() {
        validate_plugin_name(OsStr::new("工具箱_1.0_作者")).unwrap();
    }

    #[test]
    fn rejects_paths_as_plugin_names() {
        for value in ["", ".", "..", "a/b", r"a\b", r"C:\plugin"] {
            assert_eq!(
                validate_plugin_name(OsStr::new(value)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn cleaning_one_unit_preserves_other_repository_units() {
        let (root, paths) = test_paths();
        let repository = root.join("repo/Edgeless/BoostRepo");
        let selected = repository.join("工具箱");
        let preserved = repository.join("保留插件");
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&preserved).unwrap();
        fs::create_dir_all(paths.plugin_info.join("File")).unwrap();
        fs::create_dir_all(&paths.edgeless).unwrap();
        fs::write(selected.join("payload.dll"), "payload").unwrap();
        fs::write(paths.edgeless.join("payload.dll"), "payload").unwrap();
        fs::write(paths.plugin_info.join("File/工具箱.txt"), "payload.dll\n").unwrap();
        fs::write(
            paths.plugin_info.join("List_LocalBoost.txt"),
            "工具箱\n保留插件\n",
        )
        .unwrap();

        let result = clean_unit(&paths, &repository, OsStr::new("工具箱"));

        result.result.unwrap();
        assert!(!selected.exists());
        assert!(preserved.is_dir());
        assert!(!paths.edgeless.join("payload.dll").exists());
        assert_eq!(
            fs::read_to_string(paths.plugin_info.join("List_LocalBoost.txt")).unwrap(),
            "保留插件\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
