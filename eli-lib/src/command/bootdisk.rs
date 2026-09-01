use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// An Edgeless boot disk found on the current computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDisk {
    /// The platform-specific partition identifier, such as `U:` or `/dev/sdb1`.
    pub partition: PathBuf,
    /// The platform-specific mount point of the partition.
    pub mount_point: PathBuf,
    /// The complete contents of `Edgeless/version.txt`.
    pub version: String,
}

/// How a boot disk was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDiskSelectionSource {
    /// The caller explicitly supplied a partition identifier or mount point.
    Explicit,
    /// The library selected a disk from the discovered candidates.
    Automatic,
}

/// The result of selecting one Edgeless boot disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDiskSelection {
    /// The selected boot disk.
    pub selected: BootDisk,
    /// All candidates considered by automatic selection.
    ///
    /// Explicit selection contains only the selected disk.
    pub candidates: Vec<BootDisk>,
    /// Whether selection was explicit or automatic.
    pub source: BootDiskSelectionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountedPartition {
    identifier: PathBuf,
    mount_point: PathBuf,
}

/// Scans all mounted partitions for Edgeless boot disks.
///
/// This is the library command entry point used by the CLI.
pub fn list() -> io::Result<Vec<BootDisk>> {
    let readable_partitions = mounted_partitions()?
        .into_iter()
        .filter(|partition| fs::read_dir(&partition.mount_point).is_ok());
    list_from_partitions(readable_partitions)
}

/// Selects one Edgeless boot disk.
///
/// An explicitly supplied partition identifier or mount point always wins and
/// must resolve to a readable `Edgeless/version.txt`. Automatic selection
/// chooses the greatest Windows drive letter, or the lexicographically greatest
/// mount path on Linux and macOS.
pub fn get(preferred_partition: Option<&Path>) -> io::Result<BootDiskSelection> {
    if let Some(partition) = preferred_partition {
        return select_explicit_boot_disk(partition);
    }

    select_boot_disk(list()?)
}

fn select_explicit_boot_disk(partition: &Path) -> io::Result<BootDiskSelection> {
    let requested = normalize_partition_identifier(partition);
    let mounted_partitions = mounted_partitions()?;
    let partition = resolve_preferred_partition(&requested, &mounted_partitions)?;

    let selected = read_boot_disk(&partition)?;
    Ok(BootDiskSelection {
        candidates: vec![selected.clone()],
        selected,
        source: BootDiskSelectionSource::Explicit,
    })
}

fn resolve_preferred_partition(
    requested: &Path,
    mounted_partitions: &[MountedPartition],
) -> io::Result<MountedPartition> {
    let matching_partition = mounted_partitions.iter().find(|partition| {
        normalize_partition_identifier(&partition.identifier) == requested
            || normalize_partition_identifier(&partition.mount_point) == requested
    });

    match matching_partition {
        Some(partition) => Ok(partition.clone()),
        None if is_device_identifier(requested) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "boot disk partition is not mounted or was not found: {}",
                requested.display()
            ),
        )),
        None => Ok(MountedPartition {
            identifier: requested.to_owned(),
            mount_point: requested.to_owned(),
        }),
    }
}

#[cfg(windows)]
fn mounted_partitions() -> io::Result<Vec<MountedPartition>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // SAFETY: `GetLogicalDrives` takes no arguments and only returns a bitmask.
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

fn list_from_partitions<I>(partitions: I) -> io::Result<Vec<BootDisk>>
where
    I: IntoIterator<Item = MountedPartition>,
{
    let mut boot_disks = Vec::new();

    for partition in partitions {
        match read_boot_disk(&partition) {
            Ok(boot_disk) => boot_disks.push(boot_disk),
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

fn select_boot_disk(mut candidates: Vec<BootDisk>) -> io::Result<BootDiskSelection> {
    candidates.sort_unstable_by(|left, right| right.mount_point.cmp(&left.mount_point));
    let selected = candidates
        .first()
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Edgeless boot disk found"))?;

    Ok(BootDiskSelection {
        selected,
        candidates,
        source: BootDiskSelectionSource::Automatic,
    })
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

        let result = list_from_partitions([
            MountedPartition {
                identifier: drive_c.clone(),
                mount_point: drive_c.clone(),
            },
            MountedPartition {
                identifier: drive_d.clone(),
                mount_point: drive_d,
            },
        ])
        .unwrap();

        assert_eq!(
            result,
            vec![BootDisk {
                partition: drive_c.clone(),
                mount_point: drive_c,
                version: "1.2.3\n".to_owned(),
            }]
        );

        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn automatic_selection_chooses_the_greatest_mount_point() {
        let candidates = vec![
            BootDisk {
                partition: PathBuf::from("/dev/a"),
                mount_point: PathBuf::from("/media/A"),
                version: "a".to_owned(),
            },
            BootDisk {
                partition: PathBuf::from("/dev/z"),
                mount_point: PathBuf::from("/media/Z"),
                version: "z".to_owned(),
            },
        ];

        let result = select_boot_disk(candidates).unwrap();

        assert_eq!(result.selected.mount_point, PathBuf::from("/media/Z"));
        assert_eq!(result.candidates.len(), 2);
        assert_eq!(result.source, BootDiskSelectionSource::Automatic);
    }

    #[test]
    fn automatic_selection_fails_when_no_boot_disk_exists() {
        let error = select_boot_disk(Vec::new()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn explicit_selection_uses_the_requested_mount_point() {
        let test_root = std::env::temp_dir().join(format!(
            "eli-lib-explicit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(test_root.join("Edgeless")).unwrap();
        fs::write(
            test_root.join("Edgeless").join("version.txt"),
            "explicit-version",
        )
        .unwrap();

        let result = select_explicit_boot_disk(&test_root).unwrap();

        assert_eq!(result.selected.mount_point, test_root);
        assert_eq!(result.selected.version, "explicit-version");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.source, BootDiskSelectionSource::Explicit);

        fs::remove_dir_all(result.selected.mount_point).unwrap();
    }

    #[test]
    fn resolves_a_device_identifier_with_a_trailing_separator() {
        let partition = MountedPartition {
            identifier: PathBuf::from("/dev/eli-test"),
            mount_point: PathBuf::from("/media/edgeless"),
        };
        let requested = normalize_partition_identifier(Path::new("/dev/eli-test/"));

        let result =
            resolve_preferred_partition(&requested, std::slice::from_ref(&partition)).unwrap();

        assert_eq!(result, partition);
    }

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
