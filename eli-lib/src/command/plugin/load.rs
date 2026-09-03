#![cfg_attr(not(windows), allow(dead_code))]

use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(windows)]
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalBoostHandling {
    Ignore,
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOptions {
    pub recursive: bool,
    pub jobs: NonZeroUsize,
    pub local_boost: LocalBoostHandling,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            jobs: NonZeroUsize::new(2).expect("the default job count is non-zero"),
            local_boost: LocalBoostHandling::Ignore,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Loaded,
    LoadedWithLocalBoost,
    SkippedLocalBoost,
}

#[derive(Debug)]
pub struct LoadResult {
    pub path: PathBuf,
    pub result: Result<LoadStatus, io::Error>,
}

#[derive(Debug)]
pub struct LoadSummary {
    pub results: Vec<LoadResult>,
}

impl LoadSummary {
    pub fn succeeded(&self) -> usize {
        self.results
            .iter()
            .filter(|result| {
                matches!(
                    result.result,
                    Ok(LoadStatus::Loaded | LoadStatus::LoadedWithLocalBoost)
                )
            })
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.result.is_err())
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.result, Ok(LoadStatus::SkippedLocalBoost)))
            .count()
    }

    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

#[derive(Debug, Clone)]
struct RuntimePaths {
    edgeless: PathBuf,
    plugin_release: PathBuf,
    installers: PathBuf,
    plugin_info: PathBuf,
}

impl RuntimePaths {
    #[cfg(windows)]
    fn detect() -> io::Result<Self> {
        let program_files = env::var_os("ProgramFiles").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "ProgramFiles is not set in the Windows PE environment",
            )
        })?;
        let system_drive = env::var_os("SystemDrive").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "SystemDrive is not set in the Windows PE environment",
            )
        })?;
        let edgeless = PathBuf::from(program_files).join("Edgeless");
        let system_drive = system_drive_root(&system_drive);
        Ok(Self {
            plugin_release: edgeless.join("plugin_release"),
            installers: edgeless.join("安装程序"),
            plugin_info: system_drive.join("Users").join("Plugins_info"),
            edgeless,
        })
    }
}

#[cfg(windows)]
fn system_drive_root(system_drive: &OsStr) -> PathBuf {
    let mut root = PathBuf::from(system_drive);
    if !root.has_root() {
        root.push("\\");
    }
    root
}

trait PackageLoader: Send + Sync {
    fn extract(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn run_cmd(&self, script: &Path, working_directory: &Path) -> io::Result<()>;
    fn run_wcs(&self, script: &Path, working_directory: &Path) -> io::Result<()>;
    fn load_with_local_boost(&self, source: &Path) -> io::Result<()>;
}

#[derive(Debug)]
#[cfg(windows)]
struct SystemPackageLoader {
    seven_zip: PathBuf,
    cmd: PathBuf,
    pecmd: PathBuf,
}

#[cfg(windows)]
impl PackageLoader for SystemPackageLoader {
    fn extract(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let mut output_argument = OsString::from("-o");
        output_argument.push(destination);
        run_checked(
            Command::new(&self.seven_zip)
                .arg("x")
                .arg(source)
                .arg("-y")
                .arg("-aos")
                .arg(output_argument),
            &format!("extract plugin package {}", source.display()),
        )
    }

    fn run_cmd(&self, script: &Path, working_directory: &Path) -> io::Result<()> {
        run_checked(
            Command::new(&self.cmd)
                .arg("/d")
                .arg("/c")
                .arg(script)
                .current_dir(working_directory),
            &format!("run plugin CMD script {}", script.display()),
        )
    }

    fn run_wcs(&self, script: &Path, working_directory: &Path) -> io::Result<()> {
        run_checked(
            Command::new(&self.pecmd)
                .arg("LOAD")
                .arg(script)
                .current_dir(working_directory),
            &format!("run plugin WCS script {}", script.display()),
        )
    }

    fn load_with_local_boost(&self, source: &Path) -> io::Result<()> {
        super::localboost::load::load_resolved(source, &self.seven_zip, &self.cmd, &self.pecmd)
    }
}

#[cfg(windows)]
pub(super) fn run_checked(command: &mut Command, operation: &str) -> io::Result<()> {
    let status = command
        .status()
        .map_err(|error| io::Error::new(error.kind(), format!("failed to {operation}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to {operation}: process exited with {status}"
        )))
    }
}

/// 加载一个或多个插件包，并汇总每个独立任务的结果。
pub fn load(ctx: &Ctx, inputs: &[PathBuf], options: LoadOptions) -> io::Result<LoadSummary> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    load_on_supported_platform(ctx, inputs, options)
}

#[cfg(windows)]
fn load_on_supported_platform(
    ctx: &Ctx,
    inputs: &[PathBuf],
    options: LoadOptions,
) -> io::Result<LoadSummary> {
    let programs = ctx.dependencies().require_programs(&[
        ProgramDependency::SevenZip,
        ProgramDependency::Cmd,
        ProgramDependency::Pecmd,
    ])?;
    let loader = Arc::new(SystemPackageLoader {
        seven_zip: programs.executable(ProgramDependency::SevenZip)?.to_owned(),
        cmd: programs.executable(ProgramDependency::Cmd)?.to_owned(),
        pecmd: programs.executable(ProgramDependency::Pecmd)?.to_owned(),
    });
    load_with(
        inputs,
        options,
        RuntimePaths::detect()?,
        loader,
        Arc::new(ProcessPublishLock::new()?),
    )
}

#[cfg(not(windows))]
fn load_on_supported_platform(
    _ctx: &Ctx,
    _inputs: &[PathBuf],
    _options: LoadOptions,
) -> io::Result<LoadSummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "plugin loading is only implemented for Windows PE",
    ))
}

fn load_with(
    inputs: &[PathBuf],
    options: LoadOptions,
    paths: RuntimePaths,
    loader: Arc<dyn PackageLoader>,
    process_lock: Arc<ProcessPublishLock>,
) -> io::Result<LoadSummary> {
    let expanded = expand_inputs(inputs, options.recursive);
    let mut results = expanded.errors;
    let mut tasks = VecDeque::new();
    let mut files_by_name = HashMap::<String, Vec<usize>>::new();
    let mut candidates = Vec::new();
    for indexed in expanded.files {
        if is_local_boost(&indexed.path) && options.local_boost == LocalBoostHandling::Ignore {
            results.push(IndexedResult {
                index: indexed.index,
                path: indexed.path,
                result: Ok(LoadStatus::SkippedLocalBoost),
            });
            continue;
        }
        match plugin_name(&indexed.path) {
            Ok(name) => {
                files_by_name
                    .entry(name.to_string_lossy().to_lowercase())
                    .or_default()
                    .push(indexed.index);
                candidates.push(indexed);
            }
            Err(error) => results.push(IndexedResult {
                index: indexed.index,
                path: indexed.path,
                result: Err(error),
            }),
        }
    }
    let conflicting = files_by_name
        .values()
        .filter(|indexes| indexes.len() > 1)
        .flatten()
        .copied()
        .collect::<HashSet<_>>();
    for indexed in candidates {
        if conflicting.contains(&indexed.index) {
            results.push(IndexedResult {
                index: indexed.index,
                path: indexed.path,
                result: Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "multiple input packages have the same file stem",
                )),
            });
            continue;
        }
        tasks.push_back(indexed);
    }

    let shared_tasks = Arc::new(Mutex::new(tasks));
    let shared_results = Arc::new(Mutex::new(Vec::new()));
    let publish_lock = Arc::new(Mutex::new(()));
    let worker_count = options
        .jobs
        .get()
        .min(shared_tasks.lock().expect("task queue lock").len());

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let tasks = Arc::clone(&shared_tasks);
            let worker_results = Arc::clone(&shared_results);
            let loader = Arc::clone(&loader);
            let paths = paths.clone();
            let publish_lock = Arc::clone(&publish_lock);
            let process_lock = Arc::clone(&process_lock);
            scope.spawn(move || {
                loop {
                    let task = match tasks.lock() {
                        Ok(mut tasks) => tasks.pop_front(),
                        Err(_) => None,
                    };
                    let Some(task) = task else {
                        break;
                    };
                    let result = if is_local_boost(&task.path) {
                        loader
                            .load_with_local_boost(&task.path)
                            .map(|_| LoadStatus::LoadedWithLocalBoost)
                    } else {
                        load_one(
                            &task.path,
                            &paths,
                            loader.as_ref(),
                            &publish_lock,
                            &process_lock,
                        )
                        .map(|_| LoadStatus::Loaded)
                    };
                    if let Ok(mut results) = worker_results.lock() {
                        results.push(IndexedResult {
                            index: task.index,
                            path: task.path,
                            result,
                        });
                    }
                }
            });
        }
    });

    let mut worker_results = Arc::into_inner(shared_results)
        .expect("all worker result references were dropped")
        .into_inner()
        .map_err(|_| io::Error::other("plugin result lock was poisoned"))?;
    results.append(&mut worker_results);
    results.sort_unstable_by_key(|result| result.index);
    Ok(LoadSummary {
        results: results
            .into_iter()
            .map(|result| LoadResult {
                path: result.path,
                result: result.result,
            })
            .collect(),
    })
}

fn load_one(
    source: &Path,
    paths: &RuntimePaths,
    loader: &dyn PackageLoader,
    publish_lock: &Mutex<()>,
    process_lock: &ProcessPublishLock,
) -> io::Result<()> {
    if !fs::metadata(source).is_ok_and(|metadata| metadata.is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("plugin package is not a file: {}", source.display()),
        ));
    }
    let plugin_name = plugin_name(source)?;
    fs::create_dir_all(&paths.plugin_release)?;
    let staging = unique_staging_path(&paths.plugin_release, &plugin_name);
    fs::create_dir(&staging)?;
    let result = (|| {
        loader.extract(source, &staging)?;
        reject_reparse_points(&staging)?;
        let inventory = inventory(&staging)?;
        let published = {
            let _thread_guard = publish_lock
                .lock()
                .map_err(|_| io::Error::other("plugin publish lock was poisoned"))?;
            let _process_guard = process_lock.acquire()?;
            publish(&plugin_name, &staging, inventory, paths)?
        };
        run_scripts(&published, paths, loader)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

pub(super) fn plugin_name(source: &Path) -> io::Result<OsString> {
    let name = source.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin package has no file stem: {}", source.display()),
        )
    })?;
    let path = Path::new(name);
    let mut components = path.components();
    if name.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin package has an unsafe name: {}", source.display()),
        ));
    }
    Ok(name.to_owned())
}

fn unique_staging_path(parent: &Path, plugin_name: &OsStr) -> PathBuf {
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".eli-{}-{}-{}",
        plugin_name.to_string_lossy(),
        std::process::id(),
        sequence
    ))
}

#[derive(Debug)]
struct Inventory {
    cmd: Vec<PathBuf>,
    wcs: Vec<PathBuf>,
    files: Vec<OsString>,
    directories: Vec<OsString>,
}

fn inventory(staging: &Path) -> io::Result<Inventory> {
    let mut inventory = Inventory {
        cmd: Vec::new(),
        wcs: Vec::new(),
        files: Vec::new(),
        directories: Vec::new(),
    };
    let mut entries = fs::read_dir(staging)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            inventory.directories.push(entry.file_name());
        } else if file_type.is_file() {
            let path = entry.path();
            if extension_is(&path, "cmd") {
                inventory.cmd.push(path);
            } else if extension_is(&path, "wcs") {
                inventory.wcs.push(path);
            } else {
                inventory.files.push(entry.file_name());
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported plugin entry: {}", entry.path().display()),
            ));
        }
    }
    Ok(inventory)
}

#[derive(Debug)]
struct PublishedScripts {
    cmd: Vec<PathBuf>,
    wcs: Vec<PathBuf>,
}

fn publish(
    plugin_name: &OsStr,
    staging: &Path,
    mut inventory: Inventory,
    paths: &RuntimePaths,
) -> io::Result<PublishedScripts> {
    fs::create_dir_all(&paths.edgeless)?;
    fs::create_dir_all(&paths.installers)?;
    for directory in ["Batch", "Dir", "File"] {
        fs::create_dir_all(paths.plugin_info.join(directory))?;
    }
    let counter = next_counter(&paths.plugin_info.join("Counter_Hotload.txt"))?;
    rename_scripts(&mut inventory.cmd, "cmd", counter)?;
    rename_scripts(&mut inventory.wcs, "wcs", counter)?;

    with_merge_transaction(&paths.plugin_release, |transaction| {
        let mut entries = fs::read_dir(staging)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            transaction.merge(&entry.path(), &paths.edgeless.join(entry.file_name()))?;
        }

        let cmd = final_script_paths(&inventory.cmd, &paths.edgeless)?;
        let wcs = final_script_paths(&inventory.wcs, &paths.edgeless)?;
        let batch_names = cmd
            .iter()
            .chain(&wcs)
            .filter_map(|path| path.file_name())
            .map(OsStr::to_owned)
            .collect::<Vec<_>>();
        let batch_directory = paths.plugin_info.join("Batch");
        let dir_directory = paths.plugin_info.join("Dir");
        let file_directory = paths.plugin_info.join("File");
        let list_path = paths.plugin_info.join("List_Hotload.txt");
        let snapshots = [
            FileSnapshot::capture(manifest_path(&batch_directory, plugin_name))?,
            FileSnapshot::capture(manifest_path(&dir_directory, plugin_name))?,
            FileSnapshot::capture(manifest_path(&file_directory, plugin_name))?,
            FileSnapshot::capture(list_path.clone())?,
        ];
        let metadata_result = (|| {
            write_manifest(&batch_directory, plugin_name, &batch_names)?;
            write_manifest(&dir_directory, plugin_name, &inventory.directories)?;
            write_manifest(&file_directory, plugin_name, &inventory.files)?;
            append_line(&list_path, plugin_name)
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
        Ok(PublishedScripts { cmd, wcs })
    })
}

fn rename_scripts(scripts: &mut [PathBuf], extension: &str, counter: u64) -> io::Result<()> {
    for script in scripts {
        let stem = script.file_stem().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("plugin script has no file stem: {}", script.display()),
            )
        })?;
        let new_name = format!("{}_hotload_{counter}.{extension}", stem.to_string_lossy());
        let destination = script.with_file_name(new_name);
        fs::rename(&*script, &destination)?;
        *script = destination;
    }
    Ok(())
}

fn final_script_paths(scripts: &[PathBuf], edgeless: &Path) -> io::Result<Vec<PathBuf>> {
    scripts
        .iter()
        .map(|script| {
            script
                .file_name()
                .map(|name| edgeless.join(name))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "script has no name"))
        })
        .collect()
}

fn run_scripts(
    scripts: &PublishedScripts,
    paths: &RuntimePaths,
    loader: &dyn PackageLoader,
) -> io::Result<()> {
    let mut failures = Vec::new();
    for script in &scripts.cmd {
        if let Err(error) = loader.run_cmd(script, &paths.edgeless) {
            failures.push(error.to_string());
        }
    }
    for script in &scripts.wcs {
        if let Err(error) = loader.run_wcs(script, &paths.edgeless) {
            failures.push(error.to_string());
        }
    }
    for script in scripts.cmd.iter().chain(&scripts.wcs) {
        if let Err(error) = archive_script(script, &paths.installers) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

fn archive_script(script: &Path, installers: &Path) -> io::Result<()> {
    let name = script.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("plugin script has no file name: {}", script.display()),
        )
    })?;
    merge_move(script, &installers.join(name)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to archive plugin script {}: {error}",
                script.display()
            ),
        )
    })
}

pub(super) fn next_counter(path: &Path) -> io::Result<u64> {
    let current = match fs::read_to_string(path) {
        Ok(value) => value.trim().parse::<u64>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid plugin load counter {}: {error}", path.display()),
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let next = current.checked_add(1).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "plugin load counter overflowed")
    })?;
    replace_file(path, next.to_string().as_bytes())?;
    Ok(next)
}

pub(super) fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file has no parent directory: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temporary_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("metadata"))
        .to_owned();
    temporary_name.push(format!(".eli-{}-{sequence}.tmp", std::process::id()));
    let temporary = parent.join(temporary_name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        replace_path(&temporary, path)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

pub(super) fn write_manifest(
    directory: &Path,
    plugin_name: &OsStr,
    entries: &[OsString],
) -> io::Result<()> {
    let path = manifest_path(directory, plugin_name);
    let mut lines = match fs::read_to_string(&path) {
        Ok(contents) => unique_nonempty_lines(&contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry.to_string_lossy();
        if !lines.iter().any(|line| line.eq_ignore_ascii_case(&entry)) {
            lines.push(entry.into_owned());
        }
    }
    if lines.is_empty() && !path.exists() {
        return Ok(());
    }
    let contents = lines_to_contents(&lines);
    replace_file(&path, contents.as_bytes())
}

fn manifest_path(directory: &Path, plugin_name: &OsStr) -> PathBuf {
    let mut path = directory.join(plugin_name).into_os_string();
    path.push(".txt");
    PathBuf::from(path)
}

#[derive(Debug)]
pub(super) struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl FileSnapshot {
    pub(super) fn capture(path: PathBuf) -> io::Result<Self> {
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self { path, contents })
    }

    pub(super) fn restore(&self) -> io::Result<()> {
        if let Some(contents) = &self.contents {
            replace_file(&self.path, contents)
        } else {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }
}

pub(super) fn append_line(path: &Path, value: &OsStr) -> io::Result<()> {
    let mut lines = match fs::read_to_string(path) {
        Ok(contents) => unique_nonempty_lines(&contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let value = value.to_string_lossy();
    if !lines.iter().any(|line| line.eq_ignore_ascii_case(&value)) {
        lines.push(value.into_owned());
    }
    let contents = lines_to_contents(&lines);
    replace_file(path, contents.as_bytes())
}

fn unique_nonempty_lines(contents: &str) -> Vec<String> {
    let mut lines = Vec::<String>::new();
    for line in contents.lines().filter(|line| !line.is_empty()) {
        if !lines
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(line))
        {
            lines.push(line.to_owned());
        }
    }
    lines
}

fn lines_to_contents(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

pub(super) fn merge_move(source: &Path, destination: &Path) -> io::Result<()> {
    let source_metadata = fs::symlink_metadata(source)?;
    if is_reparse_point(&source_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("plugin contains a symbolic link: {}", source.display()),
        ));
    }
    let destination_metadata = optional_symlink_metadata(destination)?;
    if destination_metadata.as_ref().is_some_and(is_reparse_point) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "plugin destination is a reparse point: {}",
                destination.display()
            ),
        ));
    }
    let source_type = source_metadata.file_type();
    if source_type.is_dir() {
        if let Some(metadata) = &destination_metadata
            && !metadata.is_dir()
        {
            fs::remove_file(destination)?;
        }
        fs::create_dir_all(destination)?;
        let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable_by_key(|entry| entry.file_name());
        for entry in entries {
            merge_move(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::remove_dir(source)?;
    } else {
        if let Some(metadata) = destination_metadata {
            if metadata.is_dir() {
                fs::remove_dir_all(destination)?;
            } else {
                fs::remove_file(destination)?;
            }
        }
        fs::rename(source, destination)?;
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct MergeTransaction {
    backup_root: PathBuf,
    changes: Vec<MergeChange>,
    finished: bool,
}

#[derive(Debug)]
enum MergeChange {
    Created(PathBuf),
    Replaced {
        destination: PathBuf,
        backup: PathBuf,
    },
}

impl MergeTransaction {
    pub(super) fn new(parent: &Path) -> io::Result<Self> {
        let backup_root = unique_staging_path(parent, OsStr::new("backup"));
        fs::create_dir(&backup_root)?;
        Ok(Self {
            backup_root,
            changes: Vec::new(),
            finished: false,
        })
    }

    pub(super) fn merge(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        let source_metadata = fs::symlink_metadata(source)?;
        if is_reparse_point(&source_metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("plugin contains a reparse point: {}", source.display()),
            ));
        }
        let destination_metadata = optional_symlink_metadata(destination)?;
        if destination_metadata.as_ref().is_some_and(is_reparse_point) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "plugin destination is a reparse point: {}",
                    destination.display()
                ),
            ));
        }

        if source_metadata.is_dir()
            && destination_metadata
                .as_ref()
                .is_some_and(fs::Metadata::is_dir)
        {
            let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
            entries.sort_unstable_by_key(|entry| entry.file_name());
            for entry in entries {
                self.merge(&entry.path(), &destination.join(entry.file_name()))?;
            }
            fs::remove_dir(source)?;
            return Ok(());
        }

        if destination_metadata.is_some() {
            let backup = self.backup_root.join(self.changes.len().to_string());
            fs::rename(destination, &backup)?;
            self.changes.push(MergeChange::Replaced {
                destination: destination.to_owned(),
                backup,
            });
        }
        fs::rename(source, destination)?;
        if destination_metadata.is_none() {
            self.changes
                .push(MergeChange::Created(destination.to_owned()));
        }
        Ok(())
    }

    pub(super) fn commit(mut self) {
        self.finished = true;
        let _ = fs::remove_dir_all(&self.backup_root);
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut failures = Vec::new();
        for change in self.changes.iter().rev() {
            match change {
                MergeChange::Created(destination) => {
                    if let Err(error) = remove_path(destination) {
                        failures.push(format!("remove {}: {error}", destination.display()));
                    }
                }
                MergeChange::Replaced {
                    destination,
                    backup,
                } => {
                    if let Err(error) = remove_path(destination) {
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

impl Drop for MergeTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}

pub(super) fn with_merge_transaction<T>(
    parent: &Path,
    operation: impl FnOnce(&mut MergeTransaction) -> io::Result<T>,
) -> io::Result<T> {
    let mut transaction = MergeTransaction::new(parent)?;
    match operation(&mut transaction) {
        Ok(value) => {
            transaction.commit();
            Ok(value)
        }
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(io::Error::new(
                error.kind(),
                format!("{error}; plugin rollback also failed: {rollback_error}"),
            )),
        },
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !is_reparse_point(&metadata) => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn optional_symlink_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(super) fn copy_file_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    let source_metadata = fs::symlink_metadata(source)?;
    if !source_metadata.is_file() || is_reparse_point(&source_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("plugin source is not a regular file: {}", source.display()),
        ));
    }
    if optional_symlink_metadata(destination)?
        .as_ref()
        .is_some_and(is_reparse_point)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "plugin destination is a reparse point: {}",
                destination.display()
            ),
        ));
    }
    fs::copy(source, destination).map(|_| ())
}

pub(super) fn reject_reparse_points(root: &Path) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_reparse_point(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "plugin archive contains a symbolic link: {}",
                        entry.path().display()
                    ),
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn is_local_boost(path: &Path) -> bool {
    extension_is(path, "7zl")
}

#[derive(Debug)]
struct IndexedPath {
    index: usize,
    path: PathBuf,
}

#[derive(Debug)]
struct IndexedResult {
    index: usize,
    path: PathBuf,
    result: Result<LoadStatus, io::Error>,
}

#[derive(Debug)]
struct ExpandedInputs {
    files: Vec<IndexedPath>,
    errors: Vec<IndexedResult>,
}

fn expand_inputs(inputs: &[PathBuf], recursive: bool) -> ExpandedInputs {
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for input in inputs {
        match fs::metadata(input) {
            Ok(metadata) if metadata.is_file() => candidates.push(input.clone()),
            Ok(metadata) if metadata.is_dir() => {
                if let Err(error) = collect_directory(input, recursive, &mut candidates) {
                    errors.push((input.clone(), error));
                }
            }
            Ok(_) => errors.push((
                input.clone(),
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "input is not a file or directory",
                ),
            )),
            Err(error) => errors.push((
                input.clone(),
                io::Error::new(
                    error.kind(),
                    format!("failed to inspect {}: {error}", input.display()),
                ),
            )),
        }
    }
    candidates.sort_unstable();
    let mut seen = HashSet::new();
    candidates.retain(|path| {
        let identity = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        seen.insert(identity)
    });

    let mut files = Vec::new();
    let mut indexed_errors = Vec::new();
    for (index, path) in candidates.into_iter().enumerate() {
        files.push(IndexedPath { index, path });
    }
    let offset = files.len();
    for (position, (path, error)) in errors.into_iter().enumerate() {
        indexed_errors.push(IndexedResult {
            index: offset + position,
            path,
            result: Err(error),
        });
    }
    ExpandedInputs {
        files,
        errors: indexed_errors,
    }
}

fn collect_directory(
    directory: &Path,
    recursive: bool,
    candidates: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_dir() && recursive {
            collect_directory(&entry.path(), true, candidates)?;
        } else if file_type.is_file()
            && ["7z", "7zl"]
                .iter()
                .any(|extension| extension_is(&entry.path(), extension))
        {
            candidates.push(entry.path());
        }
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Debug)]
pub(super) struct ProcessPublishLock {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for ProcessPublishLock {}
#[cfg(windows)]
unsafe impl Sync for ProcessPublishLock {}

#[cfg(windows)]
impl ProcessPublishLock {
    pub(super) fn new() -> io::Result<Self> {
        use std::ptr;
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = "Local\\Edgeless.eli.plugin-load.publish"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }

    pub(super) fn acquire(&self) -> io::Result<ProcessPublishGuard<'_>> {
        use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

        let status = unsafe { WaitForSingleObject(self.handle, INFINITE) };
        if status == WAIT_OBJECT_0 || status == WAIT_ABANDONED {
            Ok(ProcessPublishGuard { lock: self })
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessPublishLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
pub(super) struct ProcessPublishGuard<'a> {
    lock: &'a ProcessPublishLock,
}

#[cfg(windows)]
impl Drop for ProcessPublishGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.lock.handle);
        }
    }
}

#[cfg(not(windows))]
#[derive(Debug)]
pub(super) struct ProcessPublishLock;

#[cfg(not(windows))]
impl ProcessPublishLock {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self)
    }

    pub(super) fn acquire(&self) -> io::Result<ProcessPublishGuard<'_>> {
        Ok(ProcessPublishGuard { _lock: self })
    }
}

#[cfg(not(windows))]
pub(super) struct ProcessPublishGuard<'a> {
    _lock: &'a ProcessPublishLock,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        let root = env::temp_dir().join(format!(
            "eli-plugin-load-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[derive(Debug)]
    struct FakeLoader;

    impl PackageLoader for FakeLoader {
        fn extract(&self, source: &Path, destination: &Path) -> io::Result<()> {
            let marker = fs::read_to_string(source)?;
            if marker == "fail" {
                return Err(io::Error::other("simulated extraction failure"));
            }
            fs::create_dir_all(destination.join(marker.trim()))?;
            fs::write(destination.join("setup.cmd"), "cmd")?;
            fs::write(destination.join("setup.wcs"), "wcs")?;
            fs::write(destination.join("shared.dll"), marker)?;
            Ok(())
        }

        fn run_cmd(&self, script: &Path, _working_directory: &Path) -> io::Result<()> {
            assert!(
                script
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("_hotload_")
            );
            Ok(())
        }

        fn run_wcs(&self, script: &Path, _working_directory: &Path) -> io::Result<()> {
            assert!(
                script
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("_hotload_")
            );
            Ok(())
        }

        fn load_with_local_boost(&self, _source: &Path) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct OrderingLoader {
        events: Mutex<Vec<&'static str>>,
    }

    impl PackageLoader for OrderingLoader {
        fn extract(&self, _source: &Path, destination: &Path) -> io::Result<()> {
            fs::write(destination.join("setup.cmd"), "cmd")?;
            fs::write(destination.join("setup.wcs"), "wcs")
        }

        fn run_cmd(&self, script: &Path, _working_directory: &Path) -> io::Result<()> {
            assert!(script.with_extension("wcs").exists());
            self.events.lock().unwrap().push("cmd");
            Ok(())
        }

        fn run_wcs(&self, script: &Path, _working_directory: &Path) -> io::Result<()> {
            assert!(script.with_extension("cmd").exists());
            self.events.lock().unwrap().push("wcs");
            Ok(())
        }

        fn load_with_local_boost(&self, _source: &Path) -> io::Result<()> {
            unreachable!()
        }
    }

    #[derive(Debug)]
    struct ConcurrentLoader {
        active: AtomicUsize,
        maximum: AtomicUsize,
        arrivals: (Mutex<usize>, Condvar),
    }

    impl PackageLoader for ConcurrentLoader {
        fn extract(&self, _source: &Path, destination: &Path) -> io::Result<()> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            let (arrivals, changed) = &self.arrivals;
            let mut arrivals = arrivals.lock().unwrap();
            *arrivals += 1;
            changed.notify_all();
            let (arrivals, timeout) = changed
                .wait_timeout_while(arrivals, Duration::from_secs(2), |arrivals| *arrivals < 2)
                .unwrap();
            if timeout.timed_out() && *arrivals < 2 {
                return Err(io::Error::other("parallel extraction did not start"));
            }
            drop(arrivals);
            fs::write(destination.join("payload.dll"), "payload")?;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }

        fn run_cmd(&self, _script: &Path, _working_directory: &Path) -> io::Result<()> {
            Ok(())
        }

        fn run_wcs(&self, _script: &Path, _working_directory: &Path) -> io::Result<()> {
            Ok(())
        }

        fn load_with_local_boost(&self, _source: &Path) -> io::Result<()> {
            unreachable!()
        }
    }

    fn runtime_paths(root: &Path) -> RuntimePaths {
        let edgeless = root.join("Program Files").join("Edgeless");
        RuntimePaths {
            plugin_release: edgeless.join("plugin_release"),
            installers: edgeless.join("安装程序"),
            plugin_info: root.join("Users").join("Plugins_info"),
            edgeless,
        }
    }

    #[cfg(windows)]
    #[test]
    fn resolves_a_drive_letter_to_an_absolute_volume_root() {
        assert_eq!(system_drive_root(OsStr::new("X:")), PathBuf::from("X:\\\\"));
    }

    #[test]
    fn expands_directories_recursively_and_ignores_frozen_packages() {
        let root = test_root();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("a.7z"), "a").unwrap();
        fs::write(root.join("b.7zf"), "b").unwrap();
        fs::write(nested.join("c.7zl"), "c").unwrap();

        let flat = expand_inputs(std::slice::from_ref(&root), false);
        let recursive = expand_inputs(std::slice::from_ref(&root), true);

        assert_eq!(flat.files.len(), 1);
        assert_eq!(recursive.files.len(), 2);
        assert!(
            recursive
                .files
                .iter()
                .any(|item| item.path.ends_with("c.7zl"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn one_failed_package_does_not_stop_other_tasks() {
        let root = test_root();
        let good = root.join("good.7z");
        let bad = root.join("bad.7z");
        fs::write(&good, "GoodProgram").unwrap();
        fs::write(&bad, "fail").unwrap();

        let summary = load_with(
            &[good, bad],
            LoadOptions::default(),
            runtime_paths(&root),
            Arc::new(FakeLoader),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(summary.succeeded(), 1);
        assert_eq!(summary.failed(), 1);
        assert!(root.join("Program Files/Edgeless/GoodProgram").exists());
        assert!(
            fs::read_dir(root.join("Program Files/Edgeless/安装程序"))
                .unwrap()
                .count()
                == 2
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_boost_packages_can_be_skipped_or_delegated() {
        let root = test_root();
        let package = root.join("boost.7zl");
        fs::write(&package, "boost").unwrap();
        let paths = runtime_paths(&root);

        let skipped = load_with(
            std::slice::from_ref(&package),
            LoadOptions::default(),
            paths.clone(),
            Arc::new(FakeLoader),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();
        let delegated = load_with(
            std::slice::from_ref(&package),
            LoadOptions {
                local_boost: LocalBoostHandling::Load,
                ..LoadOptions::default()
            },
            paths,
            Arc::new(FakeLoader),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(skipped.skipped(), 1);
        assert!(matches!(
            delegated.results[0].result,
            Ok(LoadStatus::LoadedWithLocalBoost)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignored_local_boost_package_does_not_conflict_with_normal_package() {
        let root = test_root();
        let normal = root.join("same.7z");
        let local_boost = root.join("same.7zl");
        fs::write(&normal, "NormalProgram").unwrap();
        fs::write(&local_boost, "boost").unwrap();

        let summary = load_with(
            &[normal, local_boost],
            LoadOptions::default(),
            runtime_paths(&root),
            Arc::new(FakeLoader),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(summary.succeeded(), 1);
        assert_eq!(summary.skipped(), 1);
        assert_eq!(summary.failed(), 0);
        assert!(root.join("Program Files/Edgeless/NormalProgram").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_merge_through_a_destination_symbolic_link() {
        let root = test_root();
        let source = root.join("source");
        let outside = root.join("outside");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(source.join("payload.dll"), "payload").unwrap();
        create_directory_link(&outside, &destination);

        let error = merge_move(&source, &destination).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!outside.join("payload.dll").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_transaction_rolls_back_prior_payload_changes_after_failure() {
        let root = test_root();
        let source = root.join("source");
        let destination = root.join("destination");
        let outside = root.join("outside");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(source.join("first.dll"), "new").unwrap();
        fs::create_dir(source.join("second")).unwrap();
        create_directory_link(&outside, &destination.join("second"));

        {
            let mut transaction = MergeTransaction::new(&root).unwrap();
            transaction
                .merge(&source.join("first.dll"), &destination.join("first.dll"))
                .unwrap();
            assert!(
                transaction
                    .merge(&source.join("second"), &destination.join("second"))
                    .is_err()
            );
        }

        assert!(!destination.join("first.dll").exists());
        assert!(!outside.join("first.dll").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_update_cleans_existing_duplicates_and_empty_lines() {
        let root = test_root();
        let directory = root.join("Batch");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("plugin.txt"), "setup.cmd\n\nSETUP.cmd\n").unwrap();

        write_manifest(
            &directory,
            OsStr::new("plugin"),
            &[OsString::from("setup.cmd"), OsString::from("cleanup.wcs")],
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(directory.join("plugin.txt")).unwrap(),
            "setup.cmd\ncleanup.wcs\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        let status = std::process::Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn packages_with_the_same_stem_are_rejected_without_loading_either() {
        let root = test_root();
        let first_directory = root.join("first");
        let second_directory = root.join("second");
        fs::create_dir_all(&first_directory).unwrap();
        fs::create_dir_all(&second_directory).unwrap();
        let first = first_directory.join("Same.7z");
        let second = second_directory.join("same.7z");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();

        let summary = load_with(
            &[first, second],
            LoadOptions::default(),
            runtime_paths(&root),
            Arc::new(FakeLoader),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(summary.succeeded(), 0);
        assert_eq!(summary.failed(), 2);
        assert!(!root.join("Program Files/Edgeless/first").exists());
        assert!(!root.join("Program Files/Edgeless/second").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runs_cmd_before_wcs_and_archives_scripts_after_both_finish() {
        let root = test_root();
        let package = root.join("ordered_1.2_author.7z");
        fs::write(&package, "package").unwrap();
        let loader = Arc::new(OrderingLoader {
            events: Mutex::new(Vec::new()),
        });

        let summary = load_with(
            &[package],
            LoadOptions::default(),
            runtime_paths(&root),
            loader.clone(),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(summary.succeeded(), 1);
        assert_eq!(*loader.events.lock().unwrap(), ["cmd", "wcs"]);
        assert!(
            root.join("Users/Plugins_info/Batch/ordered_1.2_author.txt")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn honors_the_parallel_job_window() {
        let root = test_root();
        let first = root.join("first.7z");
        let second = root.join("second.7z");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let loader = Arc::new(ConcurrentLoader {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            arrivals: (Mutex::new(0), Condvar::new()),
        });

        let summary = load_with(
            &[first, second],
            LoadOptions {
                jobs: NonZeroUsize::new(2).unwrap(),
                ..LoadOptions::default()
            },
            runtime_paths(&root),
            loader.clone(),
            Arc::new(ProcessPublishLock::new().unwrap()),
        )
        .unwrap();

        assert_eq!(summary.succeeded(), 2);
        assert_eq!(loader.maximum.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(root).unwrap();
    }
}
