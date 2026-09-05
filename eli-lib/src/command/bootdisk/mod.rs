mod get;
mod list;

pub use get::get;
pub use list::list;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// 当前计算机上发现的 Edgeless 启动盘。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDisk {
    /// 平台特定的分区标识符，例如 `U:` 或 `/dev/sdb1`。
    pub partition: PathBuf,
    /// 平台特定的分区挂载点。
    pub mount_point: PathBuf,
    /// `Edgeless/version.txt` 的完整内容。
    pub version: String,
}

/// 启动盘的选择方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDiskSelectionSource {
    /// 调用方显式提供了分区标识符或挂载点。
    Explicit,
    /// 库从发现的候选启动盘中自动选择。
    Automatic,
}

/// 启动盘内容写入的跨进程互斥锁。
///
/// 所有会修改所选启动盘的命令都必须持有此锁，避免内核、配置和插件操作互相覆盖。
pub struct WriteLock {
    _file: File,
}

/// 获取所选启动盘的独占写入锁。
pub fn acquire_write_lock(mount_point: &Path) -> io::Result<WriteLock> {
    use fs2::FileExt;

    let path = bootdisk_lock_path(mount_point)?;
    let parent = path.parent().expect("lock file always has a parent");
    fs::create_dir_all(parent)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.lock_exclusive().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to lock boot disk writes on {}: {error}",
                mount_point.display()
            ),
        )
    })?;
    Ok(WriteLock { _file: file })
}

/// 返回用户状态目录中与启动盘一一对应的稳定锁文件路径。
fn bootdisk_lock_path(mount_point: &Path) -> io::Result<PathBuf> {
    let normalized = fs::canonicalize(mount_point).unwrap_or_else(|_| mount_point.to_owned());
    let identity = normalized.to_string_lossy();
    let identity = if cfg!(windows) {
        identity.to_ascii_lowercase()
    } else {
        identity.into_owned()
    };
    Ok(eli_state_dir()?
        .join("locks")
        .join(format!("bootdisk-{:016x}.lock", stable_hash(&identity))))
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(windows)]
fn eli_state_dir() -> io::Result<PathBuf> {
    let local_data = std::env::var_os("LOCALAPPDATA").or_else(|| {
        std::env::var_os("USERPROFILE").map(|home| {
            PathBuf::from(home)
                .join("AppData")
                .join("Local")
                .into_os_string()
        })
    });
    local_data
        .map(PathBuf::from)
        .map(|path| path.join("Edgeless").join("eli"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not available"))
}

#[cfg(target_os = "linux")]
fn eli_state_dir() -> io::Result<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME").or_else(|| {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .into_os_string()
        })
    });
    state_home
        .map(PathBuf::from)
        .map(|path| path.join("Edgeless").join("eli"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not available"))
}

#[cfg(target_os = "macos")]
fn eli_state_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| {
            path.join("Library")
                .join("Application Support")
                .join("Edgeless")
                .join("eli")
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not available"))
}

/// Edgeless 启动盘的选择结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDiskSelection {
    /// 选中的启动盘。
    pub selected: BootDisk,
    /// 自动选择时考虑的所有候选启动盘。
    ///
    /// 显式选择时只包含选中的启动盘。
    pub candidates: Vec<BootDisk>,
    /// 选择方式是显式还是自动。
    pub source: BootDiskSelectionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountedPartition {
    identifier: PathBuf,
    mount_point: PathBuf,
}

#[cfg(windows)]
fn mounted_partitions() -> io::Result<Vec<MountedPartition>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // 安全性：`GetLogicalDrives` 不接收参数，并且只返回一个位掩码。
    let drive_mask = unsafe { GetLogicalDrives() };
    if drive_mask == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((b'A'..=b'Z')
        .filter(|letter| drive_mask & (1 << (letter - b'A')) != 0)
        .map(|letter| {
            let letter = char::from(letter);
            MountedPartition {
                identifier: PathBuf::from(format!("{letter}:")),
                mount_point: PathBuf::from(format!(r"{letter}:\")),
            }
        })
        .collect())
}

#[cfg(target_os = "linux")]
fn mounted_partitions() -> io::Result<Vec<MountedPartition>> {
    let mount_info = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(parse_linux_mount_info(&mount_info))
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_mount_info(mount_info: &str) -> Vec<MountedPartition> {
    let partitions = mount_info.lines().filter_map(|line| {
        let (mount_fields, filesystem_fields) = line.split_once(" - ")?;
        let encoded_mount_point = mount_fields.split_whitespace().nth(4)?;
        let encoded_identifier = filesystem_fields.split_whitespace().nth(1)?;
        Some(MountedPartition {
            identifier: PathBuf::from(decode_mount_path(encoded_identifier)),
            mount_point: PathBuf::from(decode_mount_path(encoded_mount_point)),
        })
    });

    unique_partitions(partitions)
}

#[cfg(target_os = "macos")]
fn mounted_partitions() -> io::Result<Vec<MountedPartition>> {
    use std::process::Command;

    let output = Command::new("/sbin/mount").output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "failed to enumerate mounted partitions: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(parse_macos_mount_table(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_mount_table(mount_table: &str) -> Vec<MountedPartition> {
    let partitions = mount_table.lines().filter_map(|line| {
        let (identifier, mounted_on) = line.split_once(" on ")?;
        let (encoded_path, _) = mounted_on.rsplit_once(" (")?;
        Some(MountedPartition {
            identifier: PathBuf::from(decode_mount_path(identifier)),
            mount_point: PathBuf::from(decode_mount_path(encoded_path)),
        })
    });

    unique_partitions(partitions)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn decode_mount_path(path: &str) -> String {
    path.replace(r"\040", " ")
        .replace(r"\011", "\t")
        .replace(r"\012", "\n")
        .replace(r"\134", r"\")
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn unique_partitions<I>(partitions: I) -> Vec<MountedPartition>
where
    I: IntoIterator<Item = MountedPartition>,
{
    let mut partitions: Vec<_> = partitions.into_iter().collect();
    partitions.sort_unstable_by(|left, right| left.mount_point.cmp(&right.mount_point));
    partitions.dedup_by(|left, right| left.mount_point == right.mount_point);
    partitions
}

fn read_boot_disk(partition: &MountedPartition) -> io::Result<BootDisk> {
    if !partition.mount_point.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "boot disk path must be an absolute mount point: {}",
                partition.mount_point.display()
            ),
        ));
    }

    let version_path = partition.mount_point.join("Edgeless").join("version.txt");
    let version = fs::read_to_string(&version_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read {}: {error}", version_path.display()),
        )
    })?;

    Ok(BootDisk {
        partition: partition.identifier.clone(),
        mount_point: partition.mount_point.clone(),
        version,
    })
}

#[cfg(windows)]
fn normalize_partition_identifier(partition: &Path) -> PathBuf {
    let value = partition.to_string_lossy();
    let value = value.trim_end_matches(['\\', '/']);
    let bytes = value.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        PathBuf::from(format!("{}:", char::from(bytes[0].to_ascii_uppercase())))
    } else {
        PathBuf::from(value)
    }
}

#[cfg(not(windows))]
fn normalize_partition_identifier(partition: &Path) -> PathBuf {
    let value = partition.to_string_lossy();
    let value = if value == "/" {
        value.as_ref()
    } else {
        value.trim_end_matches('/')
    };
    PathBuf::from(value)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_device_identifier(partition: &Path) -> bool {
    partition.starts_with("/dev")
}

#[cfg(windows)]
fn is_device_identifier(_partition: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn normalizes_windows_drive_prefix_variants() {
        for value in [r"u:", "U:", "u:/"] {
            assert_eq!(
                normalize_partition_identifier(Path::new(value)),
                PathBuf::from("U:")
            );
        }
    }

    #[test]
    fn stores_bootdisk_locks_outside_the_bootdisk_root() {
        let mount_point = Path::new("eli-lock-path-test");
        let path = bootdisk_lock_path(mount_point).unwrap();

        assert_ne!(path.parent(), mount_point.parent());
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("bootdisk-") && name.ends_with(".lock"))
        );
    }

    #[test]
    fn decodes_escaped_mount_paths() {
        assert_eq!(
            decode_mount_path(r"/media/Edgeless\040USB"),
            "/media/Edgeless USB"
        );
    }

    #[test]
    fn parses_linux_mount_info_and_removes_duplicates() {
        let mount_info = concat!(
            "22 1 8:1 / / rw,relatime - ext4 /dev/sda1 rw\n",
            "23 1 8:2 / /media/Edgeless\\040USB rw,relatime - vfat /dev/sdb1 rw\n",
            "24 1 8:2 / /media/Edgeless\\040USB rw,relatime - vfat /dev/sdb1 rw\n",
        );

        assert_eq!(
            parse_linux_mount_info(mount_info),
            vec![
                MountedPartition {
                    identifier: PathBuf::from("/dev/sda1"),
                    mount_point: PathBuf::from("/"),
                },
                MountedPartition {
                    identifier: PathBuf::from("/dev/sdb1"),
                    mount_point: PathBuf::from("/media/Edgeless USB"),
                },
            ]
        );
    }

    #[test]
    fn parses_macos_mount_table() {
        let mount_table = concat!(
            "/dev/disk3s1 on /Volumes/Edgeless USB (msdos, local)\n",
            "/dev/disk1s1 on / (apfs, local, read-only)\n",
        );

        assert_eq!(
            parse_macos_mount_table(mount_table),
            vec![
                MountedPartition {
                    identifier: PathBuf::from("/dev/disk1s1"),
                    mount_point: PathBuf::from("/"),
                },
                MountedPartition {
                    identifier: PathBuf::from("/dev/disk3s1"),
                    mount_point: PathBuf::from("/Volumes/Edgeless USB"),
                },
            ]
        );
    }
}
