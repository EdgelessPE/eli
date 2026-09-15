// `.jpg` 壁纸组件。
//
// 独立 `.jpg` 或 `.eth` 中的 `WallPaper.jpg` 必须先按内容解码为有效静态 JPEG；
// 应用仍交给 PECMD 的 `WALL` 命令，直接传会话稳定路径，不猜测 PECMD 内部行为。

use std::io;
use std::path::{Path, PathBuf};

use super::transaction::FileSnapshot;
use super::{ThemeBackend, ThemePaths};

/// 壁纸预检结果：已确认可解码的来源文件。
#[derive(Debug, Clone)]
pub struct PreparedWallpaper {
    pub source: PathBuf,
}

/// 预检：内容必须是可完整解码的静态 JPEG。
pub fn prepare_wallpaper(
    source: &Path,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedWallpaper> {
    let bytes = std::fs::read(source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read wallpaper {}: {error}", source.display()),
        )
    })?;
    backend.decode_jpeg(&bytes).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "wallpaper {} is not a valid static JPEG: {error}",
                source.display()
            ),
        )
    })?;
    Ok(PreparedWallpaper {
        source: source.to_owned(),
    })
}

/// 提交：先复制到会话稳定路径，再通过 PECMD `WALL` 应用；失败时恢复旧稳定文件。
pub fn commit_wallpaper(
    prepared: &PreparedWallpaper,
    backend: &dyn ThemeBackend,
    paths: &ThemePaths,
) -> io::Result<()> {
    let stable = paths.wallpaper_file();
    super::transaction::ensure_safe_publish_path(&paths.wallpaper_dir, &stable)?;
    std::fs::create_dir_all(&paths.wallpaper_dir)?;
    super::transaction::ensure_safe_publish_path(&paths.wallpaper_dir, &stable)?;
    let snapshot = FileSnapshot::capture(&stable)?;
    let bytes = std::fs::read(&prepared.source)?;
    super::transaction::atomic_replace_bytes(&stable, &bytes)?;
    match backend.apply_wallpaper(&stable) {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut failures = Vec::new();
            if let Err(restore_error) = snapshot.restore() {
                failures.push(format!("restore wallpaper: {restore_error}"));
            }
            if failures.is_empty() {
                Err(error)
            } else {
                Err(io::Error::new(
                    error.kind(),
                    format!("{error}; {failures}", failures = failures.join("; ")),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::test_support::FakeBackend;

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-wallpaper-{label}-{}-{}",
            std::process::id(),
            super::super::transaction::unique_transaction_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn keeps_the_stable_wallpaper_after_the_prepared_source_is_removed() {
        let root = test_root("stable");
        let backend = FakeBackend::new(root.clone());
        let source = root.join("staging/wall.jpg");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"new wallpaper").unwrap();

        commit_wallpaper(
            &PreparedWallpaper {
                source: source.clone(),
            },
            &backend,
            &backend.paths,
        )
        .unwrap();
        std::fs::remove_file(source).unwrap();

        let stable = backend.paths.wallpaper_file();
        assert_eq!(std::fs::read(&stable).unwrap(), b"new wallpaper");
        assert_eq!(backend.state.lock().unwrap().wall_runs, vec![stable]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_the_previous_stable_wallpaper_when_pecmd_fails() {
        let root = test_root("rollback");
        let backend = FakeBackend::new(root.clone());
        let source = root.join("staging/wall.jpg");
        let stable = backend.paths.wallpaper_file();
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(stable.parent().unwrap()).unwrap();
        std::fs::write(&source, b"new wallpaper").unwrap();
        std::fs::write(&stable, b"old wallpaper").unwrap();
        backend.state.lock().unwrap().fail_wall = true;

        let error =
            commit_wallpaper(&PreparedWallpaper { source }, &backend, &backend.paths).unwrap_err();

        assert!(error.to_string().contains("WALL"));
        assert_eq!(std::fs::read(stable).unwrap(), b"old wallpaper");
        std::fs::remove_dir_all(root).unwrap();
    }
}
