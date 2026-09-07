use super::HookStage;
#[cfg(any(windows, test))]
use super::is_supported_script;
use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
use std::ffi::OsStr;
#[cfg(any(windows, test))]
use std::fs;
use std::io;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

/// 未指定 `--dictionary` 时使用的运行态钩子根目录。
pub const DEFAULT_DICTIONARY: &str = r"X:\Program Files\Edgeless\system_hooks";

/// 钩子脚本的调度策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallPolicy {
    /// 按文件名顺序逐个运行并等待完成。
    #[default]
    Sync,
    /// 启动所有脚本但不等待完成。
    Async,
}

/// 调用生命周期钩子时使用的选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOptions {
    pub policy: CallPolicy,
    pub dictionary: PathBuf,
}

impl Default for CallOptions {
    fn default() -> Self {
        Self {
            policy: CallPolicy::Sync,
            dictionary: PathBuf::from(DEFAULT_DICTIONARY),
        }
    }
}

/// 单个钩子脚本的启动或执行结果。
#[derive(Debug)]
pub struct HookCallResult {
    pub path: PathBuf,
    pub result: io::Result<()>,
}

/// 一次钩子调用的完整结果。
#[derive(Debug)]
pub struct HookCallSummary {
    pub results: Vec<HookCallResult>,
}

impl HookCallSummary {
    pub fn succeeded(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.result.is_ok())
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results.len() - self.succeeded()
    }

    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

#[cfg(any(windows, test))]
trait ScriptExecutor {
    fn execute(&self, script: &Path, policy: CallPolicy) -> io::Result<()>;
}

#[cfg(windows)]
#[derive(Debug)]
struct SystemScriptExecutor {
    cmd: Option<PathBuf>,
    pecmd: Option<PathBuf>,
    working_directory: PathBuf,
}

#[cfg(windows)]
impl SystemScriptExecutor {
    fn command(&self, script: &Path) -> std::process::Command {
        use std::process::{Command, Stdio};

        let mut command = if is_cmd_script(script) {
            let mut command = Command::new(
                self.cmd
                    .as_deref()
                    .expect("CMD was resolved for a discovered CMD hook script"),
            );
            command.arg("/d").arg("/c").arg(script);
            command
        } else {
            let mut command = Command::new(
                self.pecmd
                    .as_deref()
                    .expect("PECMD was resolved for a discovered WCS hook script"),
            );
            command.arg("LOAD").arg(script);
            command
        };
        command
            .current_dir(&self.working_directory)
            .stdin(Stdio::null());
        command
    }
}

#[cfg(windows)]
impl ScriptExecutor for SystemScriptExecutor {
    fn execute(&self, script: &Path, policy: CallPolicy) -> io::Result<()> {
        use std::thread;
        use std::time::Duration;

        let mut command = self.command(script);
        match policy {
            CallPolicy::Sync => {
                let status = command.status().map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("failed to run hook script {}: {error}", script.display()),
                    )
                })?;
                if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "hook script {} exited with {status}",
                        script.display()
                    )))
                }
            }
            CallPolicy::Async => {
                command.spawn().map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!("failed to start hook script {}: {error}", script.display()),
                    )
                })?;
                // 原版在每个异步脚本启动后等待 10ms，避免短时间内过度争抢 PECMD。
                thread::sleep(Duration::from_millis(10));
                Ok(())
            }
        }
    }
}

/// 调用运行态目录中的指定生命周期钩子。
pub fn call(ctx: &Ctx, hook: HookStage, options: CallOptions) -> io::Result<HookCallSummary> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    call_on_supported_platform(ctx, hook, options)
}

#[cfg(windows)]
fn call_on_supported_platform(
    ctx: &Ctx,
    hook: HookStage,
    options: CallOptions,
) -> io::Result<HookCallSummary> {
    let scripts = discover_scripts(&options.dictionary, hook.as_os_str())?;
    if scripts.is_empty() {
        return Ok(HookCallSummary {
            results: Vec::new(),
        });
    }
    let needs_cmd = scripts.iter().any(|script| is_cmd_script(script));
    let needs_pecmd = scripts.iter().any(|script| !is_cmd_script(script));
    let mut required = Vec::with_capacity(2);
    if needs_cmd {
        required.push(ProgramDependency::Cmd);
    }
    if needs_pecmd {
        required.push(ProgramDependency::Pecmd);
    }
    let programs = ctx.dependencies().require_programs(&required)?;
    let system_root = std::env::var_os("SystemRoot").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SystemRoot is not set in the Windows PE environment",
        )
    })?;
    let executor = SystemScriptExecutor {
        cmd: needs_cmd
            .then(|| {
                programs
                    .executable(ProgramDependency::Cmd)
                    .map(Path::to_owned)
            })
            .transpose()?,
        pecmd: needs_pecmd
            .then(|| {
                programs
                    .executable(ProgramDependency::Pecmd)
                    .map(Path::to_owned)
            })
            .transpose()?,
        working_directory: PathBuf::from(system_root).join("System32"),
    };
    call_scripts(scripts, options.policy, &executor)
}

#[cfg(not(windows))]
fn call_on_supported_platform(
    _ctx: &Ctx,
    _hook: HookStage,
    _options: CallOptions,
) -> io::Result<HookCallSummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "hook calling is only implemented for Windows PE",
    ))
}

#[cfg(test)]
fn call_with(
    dictionary: &Path,
    hook: &OsStr,
    policy: CallPolicy,
    executor: &dyn ScriptExecutor,
) -> io::Result<HookCallSummary> {
    let scripts = discover_scripts(dictionary, hook)?;
    call_scripts(scripts, policy, executor)
}

#[cfg(any(windows, test))]
fn discover_scripts(dictionary: &Path, hook: &OsStr) -> io::Result<Vec<PathBuf>> {
    let hook_directory = dictionary.join(hook);
    let entries = match fs::read_dir(&hook_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "failed to read hook directory {}: {error}",
                    hook_directory.display()
                ),
            ));
        }
    };
    let mut scripts = entries
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_ok_and(|file_type| file_type.is_file()) => {
                let path = entry.path();
                is_supported_script(&path).then_some(Ok(path))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<io::Result<Vec<_>>>()?;
    // CMD 与 WCS 具有相同优先级，统一按文件名排序以提供确定性。
    scripts.sort_by_key(|path| path.file_name().map(|name| name.to_ascii_lowercase()));
    Ok(scripts)
}

#[cfg(any(windows, test))]
fn call_scripts(
    scripts: Vec<PathBuf>,
    policy: CallPolicy,
    executor: &dyn ScriptExecutor,
) -> io::Result<HookCallSummary> {
    Ok(HookCallSummary {
        results: scripts
            .into_iter()
            .map(|path| HookCallResult {
                result: executor.execute(&path, policy),
                path,
            })
            .collect(),
    })
}

#[cfg(windows)]
fn is_cmd_script(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Mutex<Vec<(PathBuf, CallPolicy)>>,
    }

    impl ScriptExecutor for RecordingExecutor {
        fn execute(&self, script: &Path, policy: CallPolicy) -> io::Result<()> {
            self.calls.lock().unwrap().push((script.to_owned(), policy));
            if script.file_name() == Some(OsStr::new("b.wcs")) {
                Err(io::Error::other("expected failure"))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct ConcurrentExecutor {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        completed: Arc<AtomicUsize>,
    }

    impl ScriptExecutor for ConcurrentExecutor {
        fn execute(&self, _script: &Path, policy: CallPolicy) -> io::Result<()> {
            assert_eq!(policy, CallPolicy::Async);
            let active = Arc::clone(&self.active);
            let peak = Arc::clone(&self.peak);
            let completed = Arc::clone(&self.completed);
            thread::spawn(move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(40));
                active.fetch_sub(1, Ordering::SeqCst);
                completed.fetch_add(1, Ordering::SeqCst);
            });
            Ok(())
        }
    }

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-hook-call-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn calls_every_supported_script_in_order_and_aggregates_failures() {
        let root = test_root();
        let hook = root.join("onExit");
        fs::create_dir_all(&hook).unwrap();
        fs::write(hook.join("b.wcs"), "").unwrap();
        fs::write(hook.join("a.cmd"), "").unwrap();
        fs::write(hook.join("ignored.txt"), "").unwrap();
        let executor = RecordingExecutor::default();

        let summary = call_with(&root, OsStr::new("onExit"), CallPolicy::Async, &executor).unwrap();

        assert_eq!(summary.succeeded(), 1);
        assert_eq!(summary.failed(), 1);
        let calls = executor.calls.into_inner().unwrap();
        assert_eq!(calls[0].0.file_name(), Some(OsStr::new("a.cmd")));
        assert_eq!(calls[1].0.file_name(), Some(OsStr::new("b.wcs")));
        assert!(calls.iter().all(|call| call.1 == CallPolicy::Async));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_existing_hook_without_supported_scripts_is_a_successful_no_op() {
        let root = test_root();
        fs::create_dir_all(root.join("onExit")).unwrap();
        let summary = call_with(
            &root,
            OsStr::new("onExit"),
            CallPolicy::Sync,
            &RecordingExecutor::default(),
        )
        .unwrap();
        assert!(summary.results.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_hook_is_a_successful_no_op() {
        let root = test_root();

        let summary = call_with(
            &root,
            OsStr::new("onExit"),
            CallPolicy::Sync,
            &RecordingExecutor::default(),
        )
        .unwrap();

        assert!(summary.results.is_empty());
    }

    #[test]
    fn async_policy_allows_independent_scripts_to_overlap() {
        let executor = ConcurrentExecutor::default();
        let scripts = (0..4)
            .map(|index| PathBuf::from(format!("script-{index}.cmd")))
            .collect();

        let summary = call_scripts(scripts, CallPolicy::Async, &executor).unwrap();

        assert_eq!(summary.succeeded(), 4);
        let deadline = Instant::now() + Duration::from_secs(2);
        while executor.completed.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(executor.completed.load(Ordering::SeqCst), 4);
        assert!(executor.peak.load(Ordering::SeqCst) > 1);
    }

    #[test]
    fn default_options_use_sync_and_the_requested_runtime_dictionary() {
        let options = CallOptions::default();

        assert_eq!(options.policy, CallPolicy::Sync);
        assert_eq!(options.dictionary, Path::new(DEFAULT_DICTIONARY));
    }

    #[cfg(windows)]
    #[test]
    fn builds_originally_compatible_cmd_and_wcs_commands() {
        let executor = SystemScriptExecutor {
            cmd: Some(PathBuf::from(r"C:\runtime\cmd.exe")),
            pecmd: Some(PathBuf::from(r"C:\runtime\pecmd.exe")),
            working_directory: PathBuf::from(r"X:\Windows\System32"),
        };

        let cmd = executor.command(Path::new(r"X:\hooks\onExit\save.cmd"));
        assert_eq!(cmd.get_program(), OsStr::new(r"C:\runtime\cmd.exe"));
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("/d"),
                OsStr::new("/c"),
                OsStr::new(r"X:\hooks\onExit\save.cmd")
            ]
        );
        assert_eq!(
            cmd.get_current_dir(),
            Some(Path::new(r"X:\Windows\System32"))
        );

        let wcs = executor.command(Path::new(r"X:\hooks\onExit\save.wcs"));
        assert_eq!(wcs.get_program(), OsStr::new(r"C:\runtime\pecmd.exe"));
        assert_eq!(
            wcs.get_args().collect::<Vec<_>>(),
            [OsStr::new("LOAD"), OsStr::new(r"X:\hooks\onExit\save.wcs")]
        );
        assert_eq!(
            wcs.get_current_dir(),
            Some(Path::new(r"X:\Windows\System32"))
        );
    }
}
