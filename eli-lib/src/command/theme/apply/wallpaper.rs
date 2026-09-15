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
