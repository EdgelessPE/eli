use super::{
    BootDisk, BootDiskSelection, BootDiskSelectionSource, MountedPartition, is_device_identifier,
    list, mounted_partitions, normalize_partition_identifier, read_boot_disk,
};
use std::io;
use std::path::Path;

/// 选择一个 Edgeless 启动盘。
///
/// 显式提供的分区标识符或挂载点始终优先，并且必须能够解析到可读取的
/// `Edgeless/version.txt`。自动选择会选取 Windows 中最大的盘符，或 Linux 和
/// macOS 中按字典序排列最大的挂载路径。
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
}
