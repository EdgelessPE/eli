use crate::Ctx;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 将指定的 NesPak 必要组件包保存到选中的启动盘。
///
/// 该操作修改启动盘内容，因此仅在候选启动盘唯一或通过 `--bootdisk` 显式指定时
/// 执行。写入期间持有启动盘跨进程锁，并在目标目录中暂存完整文件后原子替换，
/// 避免其他 eli 进程读取到不完整的 `Nes_Inport.7z`。
pub fn store(ctx: &Ctx, source: &Path) -> io::Result<PathBuf> {
    validate_source(source)?;
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    store_at_path(&nespak_archive_path(&bootdisk.mount_point), source)
}

/// 构造启动盘内唯一的 NesPak 组件包位置。
fn nespak_archive_path(mount_point: &Path) -> PathBuf {
    let mut destination = mount_point.components().collect::<PathBuf>();
    destination.push("Edgeless");
    destination.push("Nes_Inport.7z");
    destination
}

fn validate_source(source: &Path) -> io::Result<()> {
    let metadata = fs::metadata(source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to inspect NesPak component archive {}: {error}",
                source.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "NesPak component archive is not a regular file: {}",
                source.display()
            ),
        ));
    }
    let extension = source.extension().and_then(OsStr::to_str);
    if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("7z")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "NesPak component archive must use the .7z extension: {}",
                source.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn store_in_edgeless_dir(edgeless_dir: &Path, source: &Path) -> io::Result<PathBuf> {
    store_at_path(&edgeless_dir.join("Nes_Inport.7z"), source)
}

fn store_at_path(destination: &Path, source: &Path) -> io::Result<PathBuf> {
    validate_source(source)?;
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "NesPak destination has no parent directory: {}",
                destination.display()
            ),
        )
    })?;
    fs::create_dir_all(parent)?;
    let destination = destination.to_owned();
    let temporary = temporary_path(&destination)?;
    let result = (|| {
        copy_to_new_file(source, &temporary)?;
        replace_path(&temporary, &destination)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| destination)
}

fn temporary_path(destination: &Path) -> io::Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "NesPak destination has no parent directory: {}",
                destination.display()
            ),
        )
    })?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        "._Inport.eli-{}-{sequence}.tmp",
        std::process::id()
    )))
}

fn copy_to_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.sync_all()
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
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
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stores_a_component_archive_at_the_fixed_destination() {
        let root = temporary_directory();
        let source = root.join("NesPak.7z");
        fs::write(&source, "package").unwrap();

        let destination = store_in_edgeless_dir(&root.join("Edgeless"), &source).unwrap();

        assert_eq!(destination, root.join("Edgeless").join("Nes_Inport.7z"));
        assert_eq!(fs::read_to_string(&source).unwrap(), "package");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "package");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builds_the_component_archive_path_with_nes_in_the_file_name() {
        let mount_point = PathBuf::from("bootdisk");

        let destination = nespak_archive_path(&mount_point);

        assert_eq!(
            destination,
            mount_point.join("Edgeless").join("Nes_Inport.7z")
        );
    }

    #[cfg(windows)]
    #[test]
    fn renders_the_windows_component_archive_path_without_a_separator_after_nes() {
        let destination = nespak_archive_path(Path::new(r"D:\"));

        assert_eq!(destination.to_string_lossy(), r"D:\Edgeless\Nes_Inport.7z");
    }

    #[test]
    fn atomically_replaces_an_existing_component_archive() {
        let root = temporary_directory();
        let source = root.join("NesPak.7z");
        let edgeless_dir = root.join("Edgeless");
        fs::create_dir_all(&edgeless_dir).unwrap();
        fs::write(&source, "new").unwrap();
        fs::write(edgeless_dir.join("Nes_Inport.7z"), "old").unwrap();

        let destination = store_in_edgeless_dir(&edgeless_dir, &source).unwrap();

        assert_eq!(fs::read_to_string(destination).unwrap(), "new");
        assert!(fs::read_dir(edgeless_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_component_archive_without_a_7z_extension() {
        let root = temporary_directory();
        let source = root.join("NesPak.zip");
        fs::write(&source, "package").unwrap();

        let error = store_in_edgeless_dir(&root.join("Edgeless"), &source).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!root.join("Edgeless").exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eli-nespak-store-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
