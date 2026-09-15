// 事务与文件发布工具。
//
// 提供：唯一事务 ID、staging 目录句柄（Drop 时清理）、同卷原子替换、
// 文件快照与恢复、Windows 语义大小写折叠和“路径仍位于根目录内”校验。
// 锁不落地为文件；主题提交互斥由 Windows named mutex 负责。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::ThemePaths;

static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 生成碰撞概率可忽略的唯一事务 ID。
pub fn unique_transaction_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("t-{nanos}-{}-{sequence}", std::process::id())
}

/// 生成碰撞概率可忽略的唯一光标目录/方案 ID；若目录已存在则内部重新生成。
pub fn unique_cursor_id(paths: &ThemePaths) -> String {
    loop {
        let id = unique_transaction_id().replace("t-", "c-");
        if !paths.cursor_root.join(&id).exists() {
            return id;
        }
    }
}

/// staging 目录句柄；离开作用域时尽力清理。
#[derive(Debug)]
pub struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    pub fn create(paths: &ThemePaths) -> io::Result<Self> {
        let path = paths.staging_root.join(unique_transaction_id());
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, leaf: &str) -> PathBuf {
        self.path.join(leaf)
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// 把已就绪文件原子发布到目标位置（同卷临时文件 + 替换）。
pub fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
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
    {
        fs::rename(source, destination)
    }
}

/// 把已就绪目录原子移动到尚不存在的目标位置。
fn atomic_move_new(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

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
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if succeeded == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

/// 已存在文件的内容快照（供当前进程内回滚）。
#[derive(Debug)]
pub struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl FileSnapshot {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let contents = match fs::read(path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            path: path.to_owned(),
            contents,
        })
    }

    pub fn restore(&self) -> io::Result<()> {
        if let Some(contents) = &self.contents {
            let parent = self.path.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("snapshot file has no parent: {}", self.path.display()),
                )
            })?;
            fs::create_dir_all(parent)?;
            write_through(&self.path, contents)
        } else {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }
}

/// 把字节内容原子写入目标文件（临时文件 + 同卷 replace）。
pub fn atomic_replace_bytes(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_through(path, contents)
}

/// 把仅含普通文件的目录先完整复制到目标同级临时目录，再以一次 rename 发布。
/// 目标必须不存在；失败时不会暴露半成品目录。
pub fn publish_directory_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("directory has no parent: {}", destination.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("directory already exists: {}", destination.display()),
        ));
    }
    let leaf = destination
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{leaf}-eli-staging-{}", unique_transaction_id()));
    let result = (|| {
        fs::create_dir(&temporary)?;
        let mut entries = fs::read_dir(source)?
            .map(|entry| entry.map(|entry| (entry.path(), entry.file_name())))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable_by(|left, right| left.1.cmp(&right.1));
        for (source_path, file_name) in entries {
            let metadata = fs::symlink_metadata(&source_path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "cursor staging contains a non-regular file: {}",
                        source_path.display()
                    ),
                ));
            }
            let target = temporary.join(file_name);
            let mut input = fs::File::open(source_path)?;
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        }
        atomic_move_new(&temporary, destination)
    })();
    if temporary.exists() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn write_through(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file has no parent: {}", path.display()),
        )
    })?;
    let temporary = parent.join(format!(
        ".{}-eli-tmp-{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        unique_transaction_id()
    ));
    let result = (|| {
        fs::create_dir_all(parent)?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        atomic_replace(&temporary, path)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Windows 不区分大小写语义的大小写折叠。
///
/// Windows 使用 `LCMapStringW` 语义；无法使用 Rust 的 ASCII 比较处理任意
/// Unicode 文件名。非 Windows 平台退化为 Unicode 大写折叠，仅用于测试与
/// 跨平台一致性校验。
pub fn case_fold(value: &str) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Globalization::{
            LCMAP_LINGUISTIC_CASING, LCMAP_UPPERCASE, LCMapStringW,
        };

        let wide = std::ffi::OsStr::new(value)
            .encode_wide()
            .collect::<Vec<_>>();
        let needed = unsafe {
            LCMapStringW(
                0,
                LCMAP_UPPERCASE | LCMAP_LINGUISTIC_CASING,
                wide.as_ptr(),
                wide.len() as i32,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return value.to_uppercase();
        }
        let mut mapped = vec![0u16; needed as usize];
        let written = unsafe {
            LCMapStringW(
                0,
                LCMAP_UPPERCASE | LCMAP_LINGUISTIC_CASING,
                wide.as_ptr(),
                wide.len() as i32,
                mapped.as_mut_ptr(),
                mapped.len() as i32,
            )
        };
        if written <= 0 {
            return value.to_uppercase();
        }
        mapped.truncate(written as usize);
        std::ffi::OsString::from_wide(&mapped)
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(windows))]
    {
        value.to_uppercase()
    }
}

/// 校验候选路径仍位于根目录内（组件级路径前缀校验）。
pub fn path_is_within(root: &Path, candidate: &Path) -> bool {
    use std::path::{Component, PathBuf};
    let root_components = root.components().collect::<Vec<_>>();
    let candidate_components = candidate.components().collect::<Vec<_>>();
    if candidate_components.len() < root_components.len() {
        return false;
    }
    for component in &candidate_components {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return false;
        }
    }
    let root_prefix = root_components.iter().collect::<PathBuf>();
    let candidate_prefix = candidate_components[..root_components.len()]
        .iter()
        .collect::<PathBuf>();
    root_prefix == candidate_prefix
}

/// 以 Windows 忽略大小写的语义比较两个文件名是否相同。
pub fn name_matches(left: &str, right: &str) -> bool {
    case_fold(left) == case_fold(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eli-theme-transaction-{}-{}",
            std::process::id(),
            unique_transaction_id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn generates_distinct_transaction_ids() {
        assert_ne!(unique_transaction_id(), unique_transaction_id());
    }

    #[test]
    fn staging_directory_is_removed_on_drop() {
        let root = test_dir();
        let paths = ThemePaths {
            system_root: root.join("vol/Windows"),
            staging_root: root.join("vol/Users/Theme/eli/staging"),
            wallpaper_dir: root.join("vol/Users/Theme/eli/wallpaper"),
            icon_root: root.join("vol/Users/Icon"),
            cursor_root: root.join("vol/Windows/Cursors/Edgeless"),
            desktop_roots: vec![],
            icon_cache_dir: root.join("cache"),
        };
        let staging = StagingDir::create(&paths).unwrap();
        let path = staging.path().to_owned();
        fs::write(staging.join("payload.txt"), "payload").unwrap();

        assert!(path.exists());
        drop(staging);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_a_snapshot_including_deletion_of_absent_targets() {
        let root = test_dir();
        let target = root.join("current.jpg");
        let snapshot = FileSnapshot::capture(&target).unwrap();
        assert!(snapshot.restore().is_ok());
        assert!(!target.exists());

        fs::write(&target, "new content").unwrap();
        let snapshot = FileSnapshot::capture(&target).unwrap();
        snapshot.restore().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new content");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folds_unicode_names_with_windows_semantics_on_windows() {
        assert_eq!(case_fold("WallPaper.jpg"), "WALLPAPER.JPG");
        assert_eq!(case_fold("møbel"), case_fold("MØBEL"));
        assert!(name_matches("aero_arrow.ANI", "Aero_Arrow.ani"));
    }

    #[test]
    fn detects_whether_a_path_stays_inside_a_root() {
        let root = Path::new("/tmp/root");
        assert!(path_is_within(root, Path::new("/tmp/root/a/b.txt")));
        assert!(path_is_within(root, Path::new("/tmp/root")));
        assert!(!path_is_within(root, Path::new("/tmp/rooted/a.txt")));
        assert!(!path_is_within(root, Path::new("/tmp/root/../a.txt")));
    }

    #[test]
    fn publishes_a_directory_without_exposing_the_staging_name() {
        let root = test_dir();
        let source = root.join("source");
        let destination = root.join("active/cursor-id");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("arrow.cur"), b"cursor").unwrap();

        publish_directory_atomically(&source, &destination).unwrap();

        assert_eq!(fs::read(destination.join("arrow.cur")).unwrap(), b"cursor");
        let siblings = fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(siblings.len(), 1);
        assert_eq!(siblings[0].path(), destination);
        fs::remove_dir_all(root).unwrap();
    }
}
