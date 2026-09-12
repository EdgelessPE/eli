use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
use std::ffi::OsString;
#[cfg(windows)]
use std::fs;
use std::io;

use super::load::LoadStatus;
#[cfg(windows)]
use super::load::restore_unit;
#[cfg(windows)]
use super::repository::{self, SelectionMode};
#[cfg(windows)]
use super::runtime::{LocalBoostLock, RuntimePaths, safe_component};

#[derive(Debug)]
pub struct StartupResult {
    pub plugin: OsString,
    pub result: Result<LoadStatus, io::Error>,
}

#[derive(Debug, Default)]
pub struct StartupSummary {
    pub results: Vec<StartupResult>,
}

impl StartupSummary {
    pub fn loaded(&self) -> usize {
        self.results
            .iter()
            .filter(|result| {
                matches!(
                    result.result,
                    Ok(LoadStatus::Loaded | LoadStatus::LoadedWithCompatibilityWarning)
                )
            })
            .count()
    }

    pub fn already_loaded(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.result, Ok(LoadStatus::AlreadyLoaded)))
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.result.is_err())
            .count()
    }

    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

/// 加载 LocalBoost 仓库中已有的全部 unit；单个失败不会阻止后续 unit。
pub fn startup(ctx: &Ctx) -> io::Result<StartupSummary> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    startup_on_supported_platform(ctx)
}

#[cfg(windows)]
fn startup_on_supported_platform(ctx: &Ctx) -> io::Result<StartupSummary> {
    let programs = ctx
        .dependencies()
        .require_programs(&[ProgramDependency::Cmd, ProgramDependency::Pecmd])?;
    let paths = RuntimePaths::detect()?;
    let Some(repository) = repository::select(&paths, SelectionMode::ExistingOnly)? else {
        return Ok(StartupSummary::default());
    };
    let lifecycle_lock = LocalBoostLock::new()?;
    let _lifecycle_guard = lifecycle_lock.acquire()?;
    super::load::reject_existing_reparse_ancestors(&repository)?;
    super::load::reject_directory_reparse_point(&repository)?;

    let mut units = fs::read_dir(&repository)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| safe_component(&entry.file_name()))
        .collect::<Vec<_>>();
    units.sort_unstable_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());

    let cmd = programs.executable(ProgramDependency::Cmd)?;
    let pecmd = programs.executable(ProgramDependency::Pecmd)?;
    let results = units
        .into_iter()
        .map(|entry| StartupResult {
            plugin: entry.file_name(),
            result: restore_unit(&entry.file_name(), &entry.path(), &paths, cmd, pecmd),
        })
        .collect();
    Ok(StartupSummary { results })
}

#[cfg(not(windows))]
fn startup_on_supported_platform(_ctx: &Ctx) -> io::Result<StartupSummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "LocalBoost startup is only implemented for Windows PE",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_startup_summary_is_successful() {
        let summary = StartupSummary::default();
        assert!(summary.is_success());
        assert_eq!(summary.loaded(), 0);
    }
}
