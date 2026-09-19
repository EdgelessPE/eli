// 统一 Shell 刷新计划。
//
// 各组件提交成功后只声明自身需求：光标重载、快捷方式通知、图标缓存失效和
// Explorer 重启。`RefreshPlan` 以“更强刷新覆盖更弱刷新”的原则合并动作，
// ESS 与 ESC 共存时只执行一次 Explorer 停止/启动周期，EIS 的普通快捷方式
// 通知可被 Explorer 重启覆盖，EMS 的 `SPI_SETCURSORS` 仍独立执行。

use std::io;
use std::path::PathBuf;

use super::ess::EssCommit;
use super::{ExecutedRefresh, ThemeBackend, ThemePaths};

/// 刷新请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRequest {
    #[allow(dead_code)]
    // 预留：EMS 的 SPI_SETCURSORS 由提交步骤直接执行；该原语供未来组件/测试使用
    CursorReload,
    ShortcutNotify(PathBuf),
    IconCacheInvalidate,
    ExplorerRestart,
    /// ESS 双 DLL 替换需要在 Explorer 停止期间执行。
    SystemIconReplacement(EssCommit),
}

/// 聚合后的最小刷新计划。
#[derive(Debug, Default)]
pub struct RefreshPlan {
    cursor_reload: bool,
    shortcut_notify: Vec<PathBuf>,
    icon_cache_invalidate: bool,
    explorer_restart: bool,
    ess: Option<EssCommit>,
}

impl RefreshPlan {
    pub fn request(&mut self, request: RefreshRequest) {
        match request {
            RefreshRequest::CursorReload => self.cursor_reload = true,
            RefreshRequest::ShortcutNotify(link) => {
                if !self.shortcut_notify.contains(&link) {
                    self.shortcut_notify.push(link);
                }
            }
            RefreshRequest::IconCacheInvalidate => self.icon_cache_invalidate = true,
            RefreshRequest::ExplorerRestart => self.explorer_restart = true,
            RefreshRequest::SystemIconReplacement(commit) => self.ess = Some(commit),
        }
    }

    /// 执行最小化后的刷新动作。
    ///
    /// `on_ess` 由编排器提供，负责在 Explorer 停止期间执行 ESS 双 DLL 替换；
    /// 其返回值会决定 ESS 组件的最终状态。无论 ESS 成功与否，已停止的
    /// Explorer 都会在 finally 路径恢复。返回的第二个值表示刷新阶段是否
    /// 整体失败（例如 ESS 替换失败或 Shell 恢复失败）；即使失败，第一个值
    /// 仍携带实际执行的刷新动作供汇总使用。
    pub fn execute_minimal(
        &self,
        backend: &dyn ThemeBackend,
        paths: &ThemePaths,
        on_ess: &mut dyn FnMut(&EssCommit) -> io::Result<()>,
    ) -> (ExecutedRefresh, io::Result<()>) {
        let mut executed = ExecutedRefresh::default();
        let restart_needed = self.explorer_restart || self.ess.is_some();
        let shell_was_running = if restart_needed {
            match backend.shell_is_running() {
                Ok(running) => running,
                Err(error) => {
                    return (
                        executed,
                        Err(io::Error::new(
                            error.kind(),
                            format!("failed to inspect Explorer before the refresh phase: {error}"),
                        )),
                    );
                }
            }
        } else {
            false
        };
        let mut failure: Option<String> = None;

        if shell_was_running && let Err(error) = backend.stop_shell() {
            return (
                executed,
                Err(io::Error::new(
                    error.kind(),
                    format!("failed to stop Explorer for the refresh phase: {error}"),
                )),
            );
        }

        if restart_needed {
            let mut ess_failed = false;
            if let Some(commit) = &self.ess {
                match on_ess(commit) {
                    Ok(()) => {
                        if self.icon_cache_invalidate {
                            match clear_icon_db_cache(paths) {
                                Ok(()) => executed.icon_cache_invalidated = true,
                                Err(error) => executed.warnings.push(error.to_string()),
                            }
                        }
                    }
                    Err(error) => {
                        ess_failed = true;
                        executed
                            .warnings
                            .push(format!("ESS replacement failed: {error}"));
                    }
                }
            } else if self.icon_cache_invalidate {
                match clear_icon_db_cache(paths) {
                    Ok(()) => executed.icon_cache_invalidated = true,
                    Err(error) => executed.warnings.push(error.to_string()),
                }
            }

            if shell_was_running {
                match backend.start_shell() {
                    Ok(()) => executed.explorer_restarted = true,
                    Err(error) => {
                        failure = Some(format!(
                            "theme resources were committed but Explorer recovery failed: {error}"
                        ));
                    }
                }
            }

            if ess_failed && failure.is_none() {
                failure = Some("ESS replacement failed and rolled back".to_owned());
            }
        }

        // 不会重启 Explorer 时，才对成功修改的快捷方式发送定点通知。
        if !restart_needed && !self.shortcut_notify.is_empty() {
            match backend.notify_shortcuts(&self.shortcut_notify) {
                Ok(()) => executed.shortcut_notified = self.shortcut_notify.len(),
                Err(error) => executed
                    .warnings
                    .push(format!("shortcut notifications failed: {error}")),
            }
        }

        if self.cursor_reload {
            match backend.refresh_cursors() {
                Ok(()) => executed.cursors_refreshed = true,
                Err(error) => {
                    executed
                        .warnings
                        .push(format!("cursor reload failed: {error}"));
                }
            }
        }

        if let Some(message) = failure {
            (executed, Err(io::Error::other(message)))
        } else {
            (executed, Ok(()))
        }
    }
}

/// 在精确图标缓存目录第一层删除普通 `*.db` 文件；不得递归、不得越界。
fn clear_icon_db_cache(paths: &ThemePaths) -> io::Result<()> {
    match super::transaction::ensure_existing_directory_not_reparse(&paths.icon_cache_dir) {
        Ok(()) => {}
        // 干净启动的精简 PE 可能尚未创建 Explorer 缓存目录；此时没有缓存
        // 需要失效，按成功处理，同时仍拒绝已存在的重解析点目录。
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let entries = match std::fs::read_dir(&paths.icon_cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut failures = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        let is_db = entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("db"));
        if !is_db {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        if !file_type.is_file() {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            failures.push(format!("{}: {error}", entry.path().display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to clear icon cache entries: {}",
            failures.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::command::theme::apply::test_support::test_root;

    fn test_paths(root: &Path) -> ThemePaths {
        ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("staging"),
            wallpaper_dir: root.join("wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: Vec::new(),
            icon_cache_dir: root.join("cache"),
        }
    }

    #[test]
    fn missing_icon_cache_directory_is_already_invalidated() {
        let root = test_root("missing-cache");
        let paths = test_paths(&root);

        assert!(clear_icon_db_cache(&paths).is_ok());
        assert!(!paths.icon_cache_dir.exists());
    }

    #[test]
    fn clears_only_first_level_regular_db_files() {
        let root = test_root("precise-cache");
        let paths = test_paths(&root);
        let nested = paths.icon_cache_dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(paths.icon_cache_dir.join("iconcache.db"), b"cache").unwrap();
        std::fs::write(paths.icon_cache_dir.join("keep.txt"), b"keep").unwrap();
        std::fs::write(nested.join("nested.db"), b"nested").unwrap();

        clear_icon_db_cache(&paths).unwrap();

        assert!(!paths.icon_cache_dir.join("iconcache.db").exists());
        assert!(paths.icon_cache_dir.join("keep.txt").exists());
        assert!(nested.join("nested.db").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
