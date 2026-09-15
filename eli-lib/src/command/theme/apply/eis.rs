// `.eis` 图标资源包。
//
// EIS 内容递归覆盖到 Edgeless 会话图标根目录（Users\Icon），`shortcut/*.ico`
// 只把可完整解码的图标纳入快捷方式映射；其他静态图片可发布。包内不接受
// 可执行文件或脚本。图标发布使用临时文件 + 同卷 replace，快捷方式修改按
// per-link best effort 逐链接提交。

use std::io;
use std::path::{Path, PathBuf};

use super::archive::{EIS_LIMITS, validate_listing, verify_extraction};
use super::refresh::RefreshPlan;
use super::transaction::case_fold;
use super::{EisStats, ThemeBackend, ThemePaths};

/// 可发布的静态图片扩展名（与 image crate 当前启用的解码器一致）。
const IMAGE_EXTENSIONS: [&str; 8] = ["ico", "bmp", "jpg", "jpeg", "png", "tga", "tiff", "webp"];

/// 快捷方式图标映射中的同名碰撞（Windows 不区分大小写）会在预检阶段拒绝。
#[derive(Debug, Clone)]
pub struct PreparedEis {
    /// 图标根目录的发布清单：相对路径（相对 `paths.icon_root`）→ staging 源文件。
    pub publish_files: Vec<(PathBuf, PathBuf)>,
    /// 快捷方式映射：图标原始基名（`.lnk` 文件名中的 stem）→ staging 源文件路径。
    pub shortcut_icons: Vec<(String, PathBuf, String)>,
}

/// 预检：完整校验嵌套归档内容并按目录/文件分类。
pub fn prepare_eis_from_archive(
    archive: &Path,
    staging: &Path,
    _paths: &ThemePaths,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedEis> {
    let entries = backend.list_archive(archive)?;
    validate_listing(&entries, &EIS_LIMITS)?;
    let whitelist = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    backend.extract_archive_entries(archive, staging, &whitelist)?;
    verify_extraction(staging)?;

    let mut publish_files = Vec::new();
    let mut shortcut_icons = Vec::new();
    let mut shortcut_stems = Vec::new();
    walk_eis_staging(
        staging,
        staging,
        backend,
        &mut publish_files,
        &mut shortcut_icons,
        &mut shortcut_stems,
    )?;
    Ok(PreparedEis {
        publish_files,
        shortcut_icons,
    })
}

fn walk_eis_staging(
    root: &Path,
    directory: &Path,
    backend: &dyn ThemeBackend,
    publish_files: &mut Vec<(PathBuf, PathBuf)>,
    shortcut_icons: &mut Vec<(String, PathBuf, String)>,
    shortcut_stems: &mut Vec<String>,
) -> io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let relative = relative_path(root, &path);
            publish_files.push((relative, path.clone()));
            walk_eis_staging(
                root,
                &path,
                backend,
                publish_files,
                shortcut_icons,
                shortcut_stems,
            )?;
            continue;
        }
        if !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "EIS archive contains a non-regular file: {}",
                    path.display()
                ),
            ));
        }
        let relative = relative_path(root, &path);
        let bytes = std::fs::read(&path)?;
        if is_shortcut_icon(&relative) {
            backend.decode_icon(&bytes).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "EIS shortcut icon cannot be fully decoded: {} ({error})",
                        path.display()
                    ),
                )
            })?;
            let stem = shortcut_stem(&relative);
            let folded = case_fold(&stem);
            if shortcut_stems.iter().any(|existing| existing == &folded) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "EIS package contains case-insensitive duplicate shortcut icons: {stem}"
                    ),
                ));
            }
            shortcut_stems.push(folded.clone());
            shortcut_icons.push((stem, path.clone(), relative.to_string_lossy().into_owned()));
            publish_files.push((relative, path));
            continue;
        }
        let extension = relative
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        if !IMAGE_EXTENSIONS.iter().any(|image| *image == extension) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("EIS archive contains a non-image file: {}", path.display()),
            ));
        }
        backend.decode_plain_image(&bytes).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "EIS image cannot be fully decoded: {} ({error})",
                    path.display()
                ),
            )
        })?;
        publish_files.push((relative, path));
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_owned()
}

fn is_shortcut_icon(relative: &Path) -> bool {
    let Some(first) = relative.components().next() else {
        return false;
    };
    let first = first.as_os_str().to_string_lossy();
    first.eq_ignore_ascii_case("shortcut")
        && relative
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ico"))
}

/// `shortcut/<stem>.ico` → 快捷方式文件名（去掉 `.lnk` 的 stem）。
fn shortcut_stem(relative: &Path) -> String {
    relative
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 提交 EIS：先把图标资源原子发布到图标根目录，再进行定点快捷方式修改。
pub fn commit_eis(
    prepared: &PreparedEis,
    backend: &dyn ThemeBackend,
    paths: &ThemePaths,
    refresh: &mut RefreshPlan,
) -> io::Result<EisStats> {
    for (relative, source) in &prepared.publish_files {
        let destination = paths.icon_root.join(relative);
        let parent = destination.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("icon has no parent directory: {}", destination.display()),
            )
        })?;
        std::fs::create_dir_all(parent)?;
        if source.is_dir() {
            continue;
        }
        super::transaction::atomic_replace(source, &destination).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to publish icon {}: {error}", destination.display()),
            )
        })?;
    }

    let mut stats = EisStats::default();
    let mut notified = Vec::new();
    for (stem, _source, relative) in &prepared.shortcut_icons {
        let links = find_links_by_stem(paths, stem)?;
        if links.is_empty() {
            stats.not_found += 1;
            continue;
        }
        let icon = paths.icon_root.join(relative);
        for link in links {
            match backend.modify_shortcut_icon(&link, &icon) {
                Ok(()) => {
                    stats.updated += 1;
                    notified.push(link);
                }
                Err(_error) => {
                    stats.failed += 1;
                }
            }
        }
    }
    for link in notified {
        refresh.request(super::refresh::RefreshRequest::ShortcutNotify(link));
    }
    Ok(stats)
}

/// 在每个桌面根目录中直接构造 `<stem>.lnk` 候选，不扫描桌面其他项目。
/// Windows 文件系统负责大小写不敏感解析；重复的根目录或候选会被去重。
fn find_links_by_stem(paths: &ThemePaths, stem: &str) -> io::Result<Vec<PathBuf>> {
    let mut seen_roots = Vec::new();
    let mut links = Vec::new();
    for root in &paths.desktop_roots {
        if seen_roots.iter().any(|seen| seen == root) {
            continue;
        }
        seen_roots.push(root.clone());
        let candidate = root.join(format!("{stem}.lnk"));
        let metadata = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && !links.iter().any(|link| link == &candidate)
        {
            links.push(candidate);
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::test_support::{FakeArchive, FakeBackend};
    use std::collections::HashMap;

    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-eis-{}-{}",
            std::process::id(),
            super::super::transaction::unique_transaction_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn paths_at(root: &Path, desktop: &Path) -> ThemePaths {
        ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("Users/Theme/eli/staging"),
            wallpaper_dir: root.join("Users/Theme/eli/wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: vec![desktop.to_owned()],
            icon_cache_dir: root.join("Cache"),
        }
    }

    fn entry(path: &str, is_directory: bool) -> super::super::archive::ArchiveEntry {
        super::super::archive::ArchiveEntry {
            path: path.to_owned(),
            size: 0,
            is_directory,
            encrypted: false,
            is_link: false,
        }
    }

    #[test]
    fn classifies_shortcut_icons_and_plain_images() {
        let root = test_root();
        let desktop = root.join("Desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        let backend = FakeBackend::new(root.join("vol"));
        let paths = paths_at(&root, &desktop);
        let archive_path = root.join("icons.eis");
        std::fs::write(&archive_path, "fake archive").unwrap();
        let mut files = HashMap::new();
        files.insert("shortcut/Tool.ico".to_owned(), vec![1, 2, 3]);
        files.insert("shortcut/App.ico".to_owned(), vec![4, 5, 6]);
        files.insert("Icon/logo.png".to_owned(), vec![7]);
        files.insert("Icon/banner.bmp".to_owned(), vec![8]);
        files.insert("Icon/logo_icon.ico".to_owned(), vec![9]);
        files.insert("shortcut/dup.ico".to_owned(), vec![10]);
        files.insert("shortcut/DUP.ico".to_owned(), vec![11]);
        backend.state.lock().unwrap().archives.insert(
            archive_path.clone(),
            FakeArchive {
                entries: vec![
                    entry("shortcut/", true),
                    entry("shortcut/Tool.ico", false),
                    entry("shortcut/App.ico", false),
                    entry("shortcut/DUP.ico", false),
                    entry("shortcut/dup.ico", false),
                    entry("Icon/logo.png", false),
                    entry("Icon/banner.bmp", false),
                    entry("Icon/logo_icon.ico", false),
                    entry("shortcut/dup.ico", false),
                ],
                files,
            },
        );

        let error =
            prepare_eis_from_archive(&archive_path, &root.join("staging/eis"), &paths, &backend)
                .unwrap_err();
        assert!(error.to_string().contains("collision"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn constructs_one_candidate_per_desktop_root_without_scanning_other_links() {
        let root = test_root();
        let first = root.join("Desktop-A");
        let second = root.join("Desktop-B");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("App.lnk"), "first").unwrap();
        std::fs::write(first.join("Unrelated.lnk"), "unrelated").unwrap();
        std::fs::write(second.join("App.lnk"), "second").unwrap();
        let mut paths = paths_at(&root, &first);
        paths.desktop_roots.push(second.clone());
        paths.desktop_roots.push(first.clone());

        let links = find_links_by_stem(&paths, "App").unwrap();

        assert_eq!(links, vec![first.join("App.lnk"), second.join("App.lnk")]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
