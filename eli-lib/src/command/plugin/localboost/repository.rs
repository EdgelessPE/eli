#![cfg_attr(not(windows), allow(dead_code))]

use crate::Ctx;
use crate::dependency::RuntimeEnvironment;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::fs::{self, OpenOptions};

#[cfg(windows)]
use super::runtime::{RuntimePaths, drive_root};

const MINIMUM_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCandidate {
    pub mount_point: PathBuf,
    pub label: String,
    pub available_bytes: u64,
    pub selectable: bool,
    pub reason: Option<String>,
    pub has_repository: bool,
}

#[derive(Debug, Clone)]
pub struct SelectionRequired {
    candidates: Vec<RepositoryCandidate>,
}

impl SelectionRequired {
    pub fn candidates(&self) -> &[RepositoryCandidate] {
        &self.candidates
    }
}

impl fmt::Display for SelectionRequired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LocalBoost repository must be selected")
    }
}

impl Error for SelectionRequired {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionMode {
    CreateIfMissing,
    ExistingOnly,
}

pub fn selection_required(error: &io::Error) -> Option<&SelectionRequired> {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<SelectionRequired>())
}

/// 验证界面返回的挂载点，并原子保存兼容旧实现的仓库分区配置。
pub fn confirm(ctx: &Ctx, mount_point: &Path) -> io::Result<PathBuf> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    confirm_on_supported_platform(mount_point)
}

#[cfg(windows)]
fn confirm_on_supported_platform(mount_point: &Path) -> io::Result<PathBuf> {
    let paths = RuntimePaths::detect()?;
    let candidate = inspect_volume(&drive_root(mount_point), &paths.system_drive)?;
    if !candidate.selectable {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            candidate
                .reason
                .unwrap_or_else(|| "LocalBoost repository volume is unavailable".to_owned()),
        ));
    }
    let repository = repository_path(&candidate.mount_point);
    fs::create_dir_all(&repository)?;
    super::load::reject_existing_reparse_ancestors(&repository)?;
    super::load::reject_directory_reparse_point(&repository)?;
    write_selection(&paths.selection_file, &candidate.mount_point)?;
    Ok(repository)
}

#[cfg(not(windows))]
fn confirm_on_supported_platform(_mount_point: &Path) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "LocalBoost repositories are only supported in Windows PE",
    ))
}

#[cfg(windows)]
pub(crate) fn select(paths: &RuntimePaths, mode: SelectionMode) -> io::Result<Option<PathBuf>> {
    if let Some(repository) = configured_repository(paths)?
        && (mode == SelectionMode::CreateIfMissing || repository.is_dir())
    {
        return Ok(Some(repository));
    }

    let candidates = discover_candidates(paths)?;
    let existing = candidates
        .iter()
        .filter(|candidate| candidate.has_repository && candidate.selectable)
        .collect::<Vec<_>>();
    if existing.len() == 1 {
        write_selection(&paths.selection_file, &existing[0].mount_point)?;
        return Ok(Some(repository_path(&existing[0].mount_point)));
    }
    if mode == SelectionMode::ExistingOnly && existing.is_empty() {
        return Ok(None);
    }

    let selection_candidates = if mode == SelectionMode::ExistingOnly {
        candidates
            .into_iter()
            .filter(|candidate| candidate.has_repository)
            .collect()
    } else {
        candidates
    };
    if !selection_candidates
        .iter()
        .any(|candidate| candidate.selectable)
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no writable LocalBoost repository volume with at least 2 GiB free space was found",
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        SelectionRequired {
            candidates: selection_candidates,
        },
    ))
}

#[cfg(windows)]
pub(crate) fn all_repositories(paths: &RuntimePaths) -> io::Result<Vec<PathBuf>> {
    let mut repositories = discover_candidates(paths)?
        .into_iter()
        .filter(|candidate| {
            candidate.has_repository
                && !same_volume(&candidate.mount_point, &paths.system_drive)
                && !candidate
                    .mount_point
                    .join("Edgeless")
                    .join("version.txt")
                    .is_file()
        })
        .map(|candidate| repository_path(&candidate.mount_point))
        .collect::<Vec<_>>();
    repositories.sort_unstable_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    repositories.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(repositories)
}

#[cfg(windows)]
fn configured_repository(paths: &RuntimePaths) -> io::Result<Option<PathBuf>> {
    let selection = match fs::read_to_string(&paths.selection_file) {
        Ok(selection) => selection,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(repository) = repository_from_selection(&selection) else {
        return Ok(None);
    };
    if same_volume(&repository, &paths.system_drive) {
        return Ok(None);
    }
    let root = drive_root(&repository);
    if !root.is_dir() {
        return Ok(None);
    }
    if root.join("Edgeless").join("version.txt").is_file() {
        return Ok(None);
    }
    super::load::reject_existing_reparse_ancestors(&repository)?;
    Ok(Some(repository))
}

#[cfg(windows)]
fn discover_candidates(paths: &RuntimePaths) -> io::Result<Vec<RepositoryCandidate>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // 安全性：该函数无入参，只返回当前已挂载盘符的位掩码。
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut candidates = Vec::new();
    for letter in b'A'..=b'Z' {
        if mask & (1 << (letter - b'A')) == 0 {
            continue;
        }
        let root = PathBuf::from(format!(r"{}:\", char::from(letter)));
        if let Ok(candidate) = inspect_volume(&root, &paths.system_drive) {
            candidates.push(candidate);
        }
    }
    candidates.sort_unstable_by_key(|candidate| candidate.mount_point.clone());
    Ok(candidates)
}

#[cfg(windows)]
fn inspect_volume(root: &Path, system_drive: &Path) -> io::Result<RepositoryCandidate> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_UNKNOWN,
    };

    let wide = wide_path(root);
    // 安全性：wide 以 NUL 结尾，并在调用期间保持有效。
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    let mut reason = if matches!(
        drive_type,
        DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR | DRIVE_REMOTE | DRIVE_CDROM
    ) {
        Some("不是可用的本地可写卷".to_owned())
    } else {
        None
    };
    if same_volume(root, system_drive) {
        reason = Some("Windows PE 系统盘不能用作 LocalBoost 仓库".to_owned());
    } else if root.join("Edgeless").join("version.txt").is_file() {
        reason = Some("Edgeless 启动盘不能用作 LocalBoost 仓库".to_owned());
    }

    let mut available = 0_u64;
    // 安全性：输出指针指向有效 u64，未使用的输出允许传空指针。
    let space_ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } != 0;
    if !space_ok && reason.is_none() {
        reason = Some("无法读取卷的可用空间".to_owned());
    } else if available < MINIMUM_FREE_BYTES && reason.is_none() {
        reason = Some("可用空间不足 2 GiB".to_owned());
    }

    if reason.is_none() && !probe_writable(root) {
        reason = Some("卷不可写".to_owned());
    }

    let mut label = vec![0_u16; 261];
    // 安全性：输入和输出缓冲区有效，长度与缓冲区一致。
    let label_ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    } != 0;
    let label = if label_ok {
        let end = label
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(label.len());
        String::from_utf16_lossy(&label[..end])
    } else {
        String::new()
    };
    Ok(RepositoryCandidate {
        mount_point: root.to_owned(),
        label,
        available_bytes: available,
        selectable: reason.is_none(),
        reason,
        has_repository: repository_path(root).is_dir(),
    })
}

#[cfg(windows)]
fn probe_writable(root: &Path) -> bool {
    let probe = root.join(format!(".eli-localboost-write-test-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            fs::remove_file(probe).is_ok()
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
fn write_selection(selection_file: &Path, mount_point: &Path) -> io::Result<()> {
    use super::super::load::replace_file;

    let root = drive_root(mount_point);
    let value = root
        .to_string_lossy()
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid repository volume"))?;
    replace_file(selection_file, value.to_string().as_bytes())
}

#[cfg(any(windows, test))]
pub(crate) fn repository_from_selection(selection: &str) -> Option<PathBuf> {
    let selection = selection.trim().trim_matches('"');
    let mut characters = selection.chars();
    let letter = characters.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let remainder = characters.as_str();
    if !remainder.is_empty() && remainder != ":" {
        return None;
    }
    Some(PathBuf::from(format!(
        r"{}:\Edgeless\BoostRepo",
        letter.to_ascii_uppercase()
    )))
}

#[cfg(windows)]
fn repository_path(root: &Path) -> PathBuf {
    root.join("Edgeless").join("BoostRepo")
}

#[cfg(windows)]
fn same_volume(left: &Path, right: &Path) -> bool {
    drive_root(left)
        .to_string_lossy()
        .eq_ignore_ascii_case(&drive_root(right).to_string_lossy())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

trait EqIgnoreAsciiCasePath {
    fn eq_ignore_ascii_case(&self, other: &Path) -> bool;
}

impl EqIgnoreAsciiCasePath for PathBuf {
    fn eq_ignore_ascii_case(&self, other: &Path) -> bool {
        self.to_string_lossy()
            .eq_ignore_ascii_case(&other.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_drive_selection_forms() {
        assert_eq!(
            repository_from_selection(" d:\r\n"),
            Some(PathBuf::from(r"D:\Edgeless\BoostRepo"))
        );
        assert_eq!(
            repository_from_selection("\"e\""),
            Some(PathBuf::from(r"E:\Edgeless\BoostRepo"))
        );
    }

    #[test]
    fn rejects_non_drive_repository_selections() {
        assert_eq!(repository_from_selection(""), None);
        assert_eq!(repository_from_selection("D:\\other"), None);
        assert_eq!(repository_from_selection("../D"), None);
    }
}
