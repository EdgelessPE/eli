use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProgramDependency {
    SevenZip,
    Cmd,
    Pecmd,
}

impl fmt::Display for ProgramDependency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(specification(*self).file_name)
    }
}

#[derive(Debug)]
struct ProgramSpecification {
    file_name: &'static str,
    probe_arguments: &'static [&'static str],
    probe_timeout: Duration,
}

fn specification(dependency: ProgramDependency) -> ProgramSpecification {
    match dependency {
        ProgramDependency::SevenZip => ProgramSpecification {
            file_name: "7z.exe",
            probe_arguments: &["i"],
            probe_timeout: Duration::from_secs(5),
        },
        ProgramDependency::Cmd => ProgramSpecification {
            file_name: "cmd.exe",
            probe_arguments: &["/d", "/c", "exit", "0"],
            probe_timeout: Duration::from_secs(5),
        },
        ProgramDependency::Pecmd => ProgramSpecification {
            file_name: "pecmd.exe",
            // PECMD 的 EXEC 参数在不同版本中存在不兼容行为；/ ? 是各版本均可
            // 无副作用执行并以成功状态退出的探测方式。
            probe_arguments: &["/?"],
            probe_timeout: Duration::from_secs(5),
        },
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPrograms {
    paths: HashMap<ProgramDependency, PathBuf>,
}

impl ResolvedPrograms {
    pub(super) fn new(paths: HashMap<ProgramDependency, PathBuf>) -> Self {
        Self { paths }
    }

    pub fn executable(&self, dependency: ProgramDependency) -> io::Result<&Path> {
        self.paths
            .get(&dependency)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("dependency was not requested: {dependency}"),
                )
            })
    }
}

pub(super) fn resolve_and_probe(dependency: ProgramDependency) -> io::Result<PathBuf> {
    let specification = specification(dependency);
    let path = find_in_path(
        OsStr::new(specification.file_name),
        env::var_os("PATH").as_deref(),
    )?;
    probe(&path, &specification)?;
    Ok(path)
}

fn find_in_path(file_name: &OsStr, path: Option<&OsStr>) -> io::Result<PathBuf> {
    let path = path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "PATH is not set; cannot find dependency {}",
                file_name.to_string_lossy()
            ),
        )
    })?;

    for directory in env::split_paths(path) {
        let candidate = directory.join(file_name);
        if fs::metadata(&candidate).is_ok_and(|metadata| metadata.is_file()) {
            return fs::canonicalize(&candidate).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "failed to resolve dependency {}: {error}",
                        candidate.display()
                    ),
                )
            });
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "dependency was not found in PATH: {}",
            file_name.to_string_lossy()
        ),
    ))
}

fn probe(path: &Path, specification: &ProgramSpecification) -> io::Result<()> {
    let mut child = Command::new(path)
        .args(specification.probe_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to test dependency {}: {error}", path.display()),
            )
        })?;
    let started = Instant::now();
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "failed to wait for dependency test {}: {error}",
                        path.display()
                    ),
                ));
            }
        };
        if let Some(status) = status {
            if status.success() {
                return Ok(());
            }
            return Err(io::Error::other(format!(
                "dependency test failed for {} with status {status}",
                path.display()
            )));
        }
        if started.elapsed() >= specification.probe_timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("dependency test timed out for {}", path.display()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "eli-dependency-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn finds_a_registered_file_in_path() {
        let first = test_dir();
        let second = test_dir();
        let executable = second.join("tool.exe");
        fs::write(&executable, "test").unwrap();
        let path = env::join_paths([&first, &second]).unwrap();

        let resolved = find_in_path(OsStr::new("tool.exe"), Some(&path)).unwrap();

        assert_eq!(resolved, fs::canonicalize(&executable).unwrap());
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn rejects_a_directory_with_the_requested_name() {
        let directory = test_dir();
        fs::create_dir(directory.join("tool.exe")).unwrap();
        let path = OsString::from(directory.as_os_str());

        let error = find_in_path(OsStr::new("tool.exe"), Some(&path)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn registered_dependencies_have_probe_commands() {
        for dependency in [
            ProgramDependency::SevenZip,
            ProgramDependency::Cmd,
            ProgramDependency::Pecmd,
        ] {
            let specification = specification(dependency);
            assert!(!specification.file_name.is_empty());
            assert!(!specification.probe_arguments.is_empty());
            assert!(!specification.probe_timeout.is_zero());
        }
    }

    #[test]
    fn probe_accepts_a_successful_process() {
        let executable = env::current_exe().unwrap();
        let specification = ProgramSpecification {
            file_name: "test-process",
            probe_arguments: &["--exact", "dependency::program::tests::probe_success_child"],
            probe_timeout: Duration::from_secs(5),
        };

        probe(&executable, &specification).unwrap();
    }

    #[test]
    fn probe_rejects_a_nonzero_exit_status() {
        let executable = env::current_exe().unwrap();
        let specification = ProgramSpecification {
            file_name: "test-process",
            probe_arguments: &[
                "--ignored",
                "--exact",
                "dependency::program::tests::probe_failure_child",
            ],
            probe_timeout: Duration::from_secs(5),
        };

        assert_eq!(
            probe(&executable, &specification).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn probe_terminates_a_process_after_timeout() {
        let executable = env::current_exe().unwrap();
        let specification = ProgramSpecification {
            file_name: "test-process",
            probe_arguments: &[
                "--ignored",
                "--exact",
                "dependency::program::tests::probe_timeout_child",
            ],
            probe_timeout: Duration::from_millis(50),
        };

        assert_eq!(
            probe(&executable, &specification).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn probe_success_child() {}

    #[test]
    #[ignore]
    fn probe_failure_child() {
        panic!("expected child-process failure");
    }

    #[test]
    #[ignore]
    fn probe_timeout_child() {
        thread::sleep(Duration::from_secs(10));
    }
}
