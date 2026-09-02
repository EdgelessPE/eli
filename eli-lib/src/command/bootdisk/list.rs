use super::{BootDisk, MountedPartition, mounted_partitions, read_boot_disk};
use std::fs;
use std::io;

/// 扫描所有已挂载分区以查找 Edgeless 启动盘。
///
/// 这是供 CLI 使用的库命令入口。
pub fn list() -> io::Result<Vec<BootDisk>> {
    let readable_partitions = mounted_partitions()?
        .into_iter()
        .filter(|partition| fs::read_dir(&partition.mount_point).is_ok());
    list_from_partitions(readable_partitions)
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
}
