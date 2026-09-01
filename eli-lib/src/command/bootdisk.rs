use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// An Edgeless boot disk found on the current computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDisk {
    /// The platform-specific mount point of the partition.
    pub mount_point: PathBuf,
    /// The complete contents of `Edgeless\\version.txt`.
    pub version: String,
}

/// Scans all mounted partitions for Edgeless boot disks.
///
/// This is the library command entry point used by the CLI.
pub fn list() -> io::Result<Vec<BootDisk>> {
    let readable_mount_points = mounted_partitions()?
        .into_iter()
        .filter(|mount_point| fs::read_dir(mount_point).is_ok());
    list_from_mount_points(readable_mount_points)
}

#[cfg(windows)]
fn mounted_partitions() -> io::Result<Vec<PathBuf>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // SAFETY: `GetLogicalDrives` takes no arguments and only returns a bitmask.
    let drive_mask = unsafe { GetLogicalDrives() };
    if drive_mask == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((b'A'..=b'Z')
        .filter(|letter| drive_mask & (1 << (letter - b'A')) != 0)
        .map(|letter| PathBuf::from(format!(r"{}:\", char::from(letter))))
        .collect())
}

#[cfg(target_os = "linux")]
fn mounted_partitions() -> io::Result<Vec<PathBuf>> {
    let mount_info = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(parse_linux_mount_info(&mount_info))
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_mount_info(mount_info: &str) -> Vec<PathBuf> {
    let mount_points = mount_info.lines().filter_map(|line| {
        let encoded_path = line.split_whitespace().nth(4)?;
        Some(PathBuf::from(decode_mount_path(encoded_path)))
    });

    unique_mount_points(mount_points)
}

#[cfg(target_os = "macos")]
fn mounted_partitions() -> io::Result<Vec<PathBuf>> {
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
fn parse_macos_mount_table(mount_table: &str) -> Vec<PathBuf> {
    let mount_points = mount_table.lines().filter_map(|line| {
        let (_, mounted_on) = line.split_once(" on ")?;
        let (encoded_path, _) = mounted_on.rsplit_once(" (")?;
        Some(PathBuf::from(decode_mount_path(encoded_path)))
    });

    unique_mount_points(mount_points)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn decode_mount_path(path: &str) -> String {
    path.replace(r"\040", " ")
        .replace(r"\011", "\t")
        .replace(r"\012", "\n")
        .replace(r"\134", r"\")
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn unique_mount_points<I>(mount_points: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut mount_points: Vec<_> = mount_points.into_iter().collect();
    mount_points.sort_unstable();
    mount_points.dedup();
    mount_points
}

fn list_from_mount_points<I, P>(mount_points: I) -> io::Result<Vec<BootDisk>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut boot_disks = Vec::new();

    for mount_point in mount_points {
        let mount_point = mount_point.as_ref();
        let version_path = mount_point.join("Edgeless").join("version.txt");
        match fs::read_to_string(version_path) {
            Ok(version) => boot_disks.push(BootDisk {
                mount_point: mount_point.to_owned(),
                version,
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(error),
        }
    }

    Ok(boot_disks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lists_only_drives_with_an_edgeless_version_file() {
        let test_root = std::env::temp_dir().join(format!(
            "eli-lib-bootdisk-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let drive_c = test_root.join("C");
        let drive_d = test_root.join("D");
        fs::create_dir_all(drive_c.join("Edgeless")).unwrap();
        fs::create_dir_all(&drive_d).unwrap();
        fs::write(drive_c.join("Edgeless").join("version.txt"), "1.2.3\n").unwrap();

        let result = list_from_mount_points([&drive_c, &drive_d]).unwrap();

        assert_eq!(
            result,
            vec![BootDisk {
                mount_point: drive_c,
                version: "1.2.3\n".to_owned(),
            }]
        );

        fs::remove_dir_all(test_root).unwrap();
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
            vec![PathBuf::from("/"), PathBuf::from("/media/Edgeless USB")]
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
            vec![PathBuf::from("/"), PathBuf::from("/Volumes/Edgeless USB")]
        );
    }
}
