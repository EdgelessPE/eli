mod environment;
mod program;

pub use environment::RuntimeEnvironment;
pub use program::{ProgramDependency, ResolvedPrograms};

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

/// 统一解析并验证命令运行所需的外部依赖。
#[derive(Debug)]
pub struct DependencyManager {
    environment: OnceLock<Result<RuntimeEnvironment, CachedError>>,
    programs: ProgramCache,
}

#[derive(Debug, Default)]
struct ProgramCache {
    seven_zip: OnceLock<Result<PathBuf, CachedError>>,
    cmd: OnceLock<Result<PathBuf, CachedError>>,
    pecmd: OnceLock<Result<PathBuf, CachedError>>,
}

impl ProgramCache {
    fn slot(&self, dependency: ProgramDependency) -> &OnceLock<Result<PathBuf, CachedError>> {
        match dependency {
            ProgramDependency::SevenZip => &self.seven_zip,
            ProgramDependency::Cmd => &self.cmd,
            ProgramDependency::Pecmd => &self.pecmd,
        }
    }
}

#[derive(Debug, Clone)]
struct CachedError {
    kind: io::ErrorKind,
    message: String,
}

impl CachedError {
    fn to_io_error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

impl DependencyManager {
    pub fn new() -> Self {
        Self {
            environment: OnceLock::new(),
            programs: ProgramCache::default(),
        }
    }

    pub fn environment(&self) -> io::Result<RuntimeEnvironment> {
        self.environment
            .get_or_init(|| {
                environment::detect().map_err(|error| CachedError {
                    kind: error.kind(),
                    message: error.to_string(),
                })
            })
            .clone()
            .map_err(|error| error.to_io_error())
    }

    pub fn require_environment(&self, required: RuntimeEnvironment) -> io::Result<()> {
        let actual = self.environment()?;
        if actual == required {
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("command requires the {required} environment, current environment is {actual}"),
        ))
    }

    pub fn require_programs(&self, required: &[ProgramDependency]) -> io::Result<ResolvedPrograms> {
        let mut resolved = HashMap::new();
        for dependency in required {
            resolved.insert(*dependency, self.require_program(*dependency)?);
        }
        Ok(ResolvedPrograms::new(resolved))
    }

    fn require_program(&self, dependency: ProgramDependency) -> io::Result<PathBuf> {
        self.programs
            .slot(dependency)
            .get_or_init(|| {
                program::resolve_and_probe(dependency).map_err(|error| CachedError {
                    kind: error.kind(),
                    message: error.to_string(),
                })
            })
            .clone()
            .map_err(|error| error.to_io_error())
    }
}

impl Default for DependencyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn rejects_a_different_runtime_environment() {
        let manager = DependencyManager::new();
        let actual = manager.environment().unwrap();
        let required = match actual {
            RuntimeEnvironment::WindowsNormal => RuntimeEnvironment::WindowsPE,
            RuntimeEnvironment::WindowsPE => RuntimeEnvironment::Linux,
            RuntimeEnvironment::Linux => RuntimeEnvironment::MacOS,
            RuntimeEnvironment::MacOS => RuntimeEnvironment::Linux,
        };

        let error = manager.require_environment(required).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains(&actual.to_string()));
        assert!(error.to_string().contains(&required.to_string()));
    }

    #[test]
    fn initializes_each_program_cache_slot_only_once_under_concurrency() {
        let cache = Arc::new(ProgramCache::default());
        let initializations = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let initializations = Arc::clone(&initializations);
            workers.push(thread::spawn(move || {
                cache
                    .slot(ProgramDependency::SevenZip)
                    .get_or_init(|| {
                        initializations.fetch_add(1, Ordering::SeqCst);
                        Ok(PathBuf::from("7z.exe"))
                    })
                    .clone()
            }));
        }

        for worker in workers {
            assert_eq!(worker.join().unwrap().unwrap(), PathBuf::from("7z.exe"));
        }
        assert_eq!(initializations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn different_programs_use_independent_cache_slots() {
        let cache = ProgramCache::default();
        cache
            .slot(ProgramDependency::SevenZip)
            .set(Ok(PathBuf::from("7z.exe")))
            .unwrap();

        assert!(cache.slot(ProgramDependency::Cmd).get().is_none());
        assert!(cache.slot(ProgramDependency::Pecmd).get().is_none());
    }
}
