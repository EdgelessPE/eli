// `eli theme startup` 两阶段启动编排。
//
// 首阶段只允许在 Explorer 首次启动前应用启动盘默认主题；对账阶段只消费
// 已发布的 EIS 图标资源，并在同一主题互斥体内修正新增快捷方式。

use crate::Ctx;
use crate::dependency::{RuntimeCapability, RuntimeEnvironment};
use std::io;

use super::apply::ApplySummary;
#[cfg(any(windows, test))]
use super::apply::{ComponentOutcome, ComponentStatus, ThemeType};

/// 启动主题入口。`reconcile=false` 为预 Shell 应用，`true` 为后置快捷方式对账。
pub fn startup(ctx: &Ctx, reconcile: bool) -> io::Result<ApplySummary> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)
        .map_err(|error| startup_error(reconcile, "environment validation", error))?;
    ctx.dependencies()
        .require_capability(RuntimeCapability::EdgelessRuntime)
        .map_err(|error| startup_error(reconcile, "environment validation", error))?;
    startup_on_supported_platform(ctx, reconcile)
}

fn startup_error(reconcile: bool, phase: &str, error: io::Error) -> io::Error {
    let command = if reconcile {
        "theme startup --reconcile"
    } else {
        "theme startup"
    };
    io::Error::new(
        error.kind(),
        format!("{command} failed during {phase}: {error}"),
    )
}

#[cfg(windows)]
fn startup_on_supported_platform(ctx: &Ctx, reconcile: bool) -> io::Result<ApplySummary> {
    use super::apply::details::windows::{ThemeApplyLock, WindowsThemeBackend};

    let backend = WindowsThemeBackend::new(ctx)
        .map_err(|error| startup_error(reconcile, "backend initialization", error))?;
    if reconcile {
        let lock = ThemeApplyLock::new()
            .map_err(|error| startup_error(true, "commit lock creation", error))?;
        let _guard = lock
            .acquire()
            .map_err(|error| startup_error(true, "commit lock acquisition", error))?;
        return reconcile_with_backend(&backend)
            .map_err(|error| startup_error(true, "shortcut reconciliation", error));
    }

    ensure_shell_not_started(&backend)
        .map_err(|error| startup_error(false, "precondition validation", error))?;
    let edgeless_dir = ctx
        .bootdisk()
        .map_err(|error| startup_error(false, "boot disk discovery", error))?
        .selected
        .mount_point
        .join("Edgeless");
    let prepared = super::apply::prepare_startup_theme(&edgeless_dir, &backend)
        .map_err(|error| startup_error(false, "precheck", error))?;
    let lock = ThemeApplyLock::new()
        .map_err(|error| startup_error(false, "commit lock creation", error))?;
    let _guard = lock
        .acquire()
        .map_err(|error| startup_error(false, "commit lock acquisition", error))?;
    // 锁外预检期间 Explorer 可能被其他启动任务拉起，因此提交前必须复查。
    ensure_shell_not_started(&backend)
        .map_err(|error| startup_error(false, "commit precondition validation", error))?;
    super::apply::commit_prepared(&prepared, &backend, super::apply::CommitMode::Startup)
        .map_err(|error| startup_error(false, "commit", error))
}

#[cfg(not(windows))]
fn startup_on_supported_platform(_ctx: &Ctx, reconcile: bool) -> io::Result<ApplySummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        if reconcile {
            "theme startup --reconcile is only implemented for Windows PE"
        } else {
            "theme startup is only implemented for Windows PE"
        },
    ))
}

#[cfg(any(windows, test))]
fn ensure_shell_not_started(backend: &dyn super::apply::details::ThemeBackend) -> io::Result<()> {
    if backend.shell_is_running()? {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Explorer is already running; use `eli theme apply` for a live session instead of the startup phase",
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn reconcile_with_backend(
    backend: &dyn super::apply::details::ThemeBackend,
) -> io::Result<ApplySummary> {
    use super::apply::ThemeComponent;
    use super::apply::details::refresh::RefreshPlan;

    let paths = backend.theme_paths()?;
    let mut refresh_plan = RefreshPlan::default();
    let (stats, failures, icon_count) = super::apply::details::eis::reconcile_published_shortcuts(
        backend,
        &paths,
        &mut refresh_plan,
    )?;
    if !backend.shell_is_running()? {
        refresh_plan.discard_shortcut_notifications();
    }
    let refresh = refresh_plan.execute_minimal(backend, &paths);
    if let Some(error) = refresh.lifecycle_error {
        return Err(error);
    }
    let mut components = Vec::new();
    if icon_count > 0 {
        let status = if stats.updated + stats.unchanged == 0 && stats.failed > 0 {
            ComponentStatus::Failed(format!(
                "every existing shortcut target failed to reconcile ({} failed): {}",
                stats.failed,
                failures.join("; ")
            ))
        } else if stats.failed > 0 {
            ComponentStatus::AppliedWithWarnings(
                std::iter::once(format!(
                    "{} shortcut link(s) failed to reconcile",
                    stats.failed
                ))
                .chain(failures)
                .collect(),
            )
        } else {
            ComponentStatus::Applied
        };
        components.push(ComponentOutcome {
            component: ThemeComponent::IconPack,
            status,
            windows_error_code: None,
        });
    }
    Ok(ApplySummary {
        source: paths.icon_root.join("shortcut"),
        outer: ThemeType::Eis,
        components,
        warnings: Vec::new(),
        eis: stats,
        refresh: refresh.executed,
    })
}

#[cfg(test)]
fn startup_with_backend(
    edgeless_dir: &std::path::Path,
    reconcile: bool,
    backend: &dyn super::apply::details::ThemeBackend,
) -> io::Result<ApplySummary> {
    if reconcile {
        return reconcile_with_backend(backend);
    }
    ensure_shell_not_started(backend)?;
    let prepared = super::apply::prepare_startup_theme(edgeless_dir, backend)?;
    ensure_shell_not_started(backend)?;
    super::apply::commit_prepared(&prepared, backend, super::apply::CommitMode::Startup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::ThemeComponent;
    use crate::command::theme::apply::test_support::{FakeBackend, test_root};

    #[test]
    fn first_phase_rejects_a_running_explorer_before_preparation() {
        let root = test_root("startup-running");
        let backend = FakeBackend::new(root.join("volume"));
        backend.state.lock().unwrap().shell_running = true;

        let error = startup_with_backend(&root.join("Edgeless"), false, &backend).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("Explorer is already running"));
        assert!(!backend.paths.staging_root.exists());
    }

    #[test]
    fn first_phase_applies_existing_wallpaper_without_starting_explorer() {
        let root = test_root("startup-wallpaper");
        let edgeless = root.join("Edgeless");
        std::fs::create_dir_all(&edgeless).unwrap();
        std::fs::write(edgeless.join("wp.jpg"), b"jpeg").unwrap();
        let backend = FakeBackend::new(root.join("volume"));

        let summary = startup_with_backend(&edgeless, false, &backend).unwrap();

        assert!(summary.is_success());
        assert_eq!(summary.components.len(), 1);
        assert_eq!(summary.components[0].component, ThemeComponent::Wallpaper);
        let state = backend.state.lock().unwrap();
        assert!(state.shell_events.is_empty());
        assert!(state.notified_links.is_empty());
    }

    #[test]
    fn one_precheck_failure_does_not_block_an_independent_component() {
        let root = test_root("startup-partial-precheck");
        let edgeless = root.join("Edgeless");
        std::fs::create_dir_all(edgeless.join("Default")).unwrap();
        std::fs::write(edgeless.join("wp.jpg"), b"jpeg").unwrap();
        std::fs::write(edgeless.join("Default/IconPack.eis"), b"invalid").unwrap();
        let backend = FakeBackend::new(root.join("volume"));

        let summary = startup_with_backend(&edgeless, false, &backend).unwrap();

        assert!(!summary.is_success());
        assert_eq!(summary.applied(), 1);
        assert_eq!(summary.failed(), 1);
        assert_eq!(backend.state.lock().unwrap().wall_runs.len(), 1);
    }

    #[test]
    fn reconcile_only_writes_changed_links_and_is_idempotent() {
        let root = test_root("startup-reconcile");
        let backend = FakeBackend::new(root.join("volume"));
        let shortcut_dir = backend.paths.icon_root.join("shortcut");
        let desktop = &backend.paths.desktop_roots[0];
        std::fs::create_dir_all(&shortcut_dir).unwrap();
        std::fs::create_dir_all(desktop).unwrap();
        std::fs::write(shortcut_dir.join("Tool.ico"), b"icon").unwrap();
        std::fs::write(desktop.join("Tool.lnk"), b"link").unwrap();
        backend.state.lock().unwrap().shell_running = true;

        let first = startup_with_backend(&root.join("unused"), true, &backend).unwrap();
        let second = startup_with_backend(&root.join("unused"), true, &backend).unwrap();

        assert_eq!(first.eis.checked, 1);
        assert_eq!(first.eis.updated, 1);
        assert_eq!(first.refresh.shortcut_notified, 1);
        assert_eq!(second.eis.checked, 1);
        assert_eq!(second.eis.updated, 0);
        assert_eq!(second.eis.unchanged, 1);
        assert_eq!(second.refresh.shortcut_notified, 0);
        let state = backend.state.lock().unwrap();
        assert_eq!(state.modified_links.len(), 1);
        assert_eq!(state.notified_links.len(), 1);
    }

    #[test]
    fn reconcile_without_published_icons_is_a_successful_no_op() {
        let root = test_root("startup-reconcile-empty");
        let backend = FakeBackend::new(root.join("volume"));
        backend.state.lock().unwrap().shell_running = true;

        let summary = startup_with_backend(&root.join("unused"), true, &backend).unwrap();

        assert!(summary.is_success());
        assert!(summary.components.is_empty());
        assert_eq!(summary.eis, Default::default());
        assert!(backend.state.lock().unwrap().modified_links.is_empty());
    }
}
