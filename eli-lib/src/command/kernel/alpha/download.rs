use super::super::{is_reparse_point, validate_wim_header};
use crate::api::edgeless::{self, AlphaDownloadInfo};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Alpha 内核下载命令的执行结果。
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadResult {
    /// 已完成 Alpha WIM 下载。
    Downloaded(PathBuf),
    /// 目标 WIM 已存在且通过校验，因此未发起下载。
    Skipped(PathBuf),
}

/// 下载最新 Alpha 内核 WIM 到调用方指定的目录。
///
/// 下载不依赖启动盘。内容会先写入目标目录内的临时文件，通过 WIM 校验后再原子发布；
/// HTTP 下载锁负责串行化相同目标的同进程和多进程并发写入。
pub fn download(directory: &Path, force: bool, token: &str) -> io::Result<DownloadResult> {
    let info = edgeless::latest_alpha_download_info(token)?;
    download_to_directory(directory, force, info, |destination, overwrite| {
        edgeless::download_latest_alpha(token, destination, overwrite, validate_wim_header)
    })
}

fn download_to_directory<F>(
    directory: &Path,
    force: bool,
    info: AlphaDownloadInfo,
    fetch: F,
) -> io::Result<DownloadResult>
where
    F: FnOnce(&Path, bool) -> io::Result<()>,
{
    fs::create_dir_all(directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to create Alpha download directory {}: {error}",
                directory.display()
            ),
        )
    })?;
    if !directory.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Alpha download directory is not a directory: {}",
                directory.display()
            ),
        ));
    }

    let destination = directory.join(info.name);
    let existed_before = inspect_destination(&destination, force)?;
    if existed_before && !force {
        return Ok(DownloadResult::Skipped(destination));
    }

    match fetch(&destination, force) {
        Ok(()) => {}
        Err(error) if !force && error.kind() == io::ErrorKind::AlreadyExists => {
            if inspect_destination(&destination, false)? {
                return Ok(DownloadResult::Skipped(destination));
            }
            return Err(error);
        }
        Err(error) => return Err(error),
    }

    if let Err(error) = validate_wim_header(&destination) {
        if !existed_before {
            let _ = fs::remove_file(&destination);
        }
        return Err(error);
    }
    Ok(DownloadResult::Downloaded(destination))
}

fn inspect_destination(destination: &Path, allow_overwrite: bool) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "existing Alpha download destination is not a regular file: {}",
                destination.display()
            ),
        ));
    }
    if !allow_overwrite {
        validate_wim_header(destination).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "existing Alpha kernel WIM is invalid; use --force to replace it {}: {error}",
                    destination.display()
                ),
            )
        })?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::super::super::WIM_MAGIC;
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn downloads_a_valid_wim_to_the_requested_directory() {
        let directory = temporary_path("download");
        let result =
            download_to_directory(&directory, false, alpha_info(), |destination, force| {
                assert!(!force);
                fs::write(destination, WIM_MAGIC)
            })
            .unwrap();
        let destination = directory.join("Edgeless_Alpha_4.1.2.wim");

        assert_eq!(result, DownloadResult::Downloaded(destination.clone()));
        assert_eq!(fs::read(destination).unwrap(), WIM_MAGIC);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn skips_an_existing_valid_wim_without_force() {
        let directory = temporary_path("skip");
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("Edgeless_Alpha_4.1.2.wim");
        fs::write(&destination, WIM_MAGIC).unwrap();

        let result = download_to_directory(&directory, false, alpha_info(), |_, _| {
            panic!("valid existing WIM must not be downloaded again")
        })
        .unwrap();

        assert_eq!(result, DownloadResult::Skipped(destination));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn force_replaces_an_existing_wim() {
        let directory = temporary_path("force");
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("Edgeless_Alpha_4.1.2.wim");
        fs::write(&destination, WIM_MAGIC).unwrap();

        let result = download_to_directory(&directory, true, alpha_info(), |path, force| {
            assert!(force);
            fs::write(path, WIM_MAGIC)
        })
        .unwrap();

        assert_eq!(result, DownloadResult::Downloaded(destination));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_an_existing_invalid_wim_without_force() {
        let directory = temporary_path("existing-invalid");
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("Edgeless_Alpha_4.1.2.wim");
        fs::write(&destination, b"broken").unwrap();

        let error = download_to_directory(&directory, false, alpha_info(), |_, _| {
            panic!("invalid existing WIM must not be silently skipped")
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("--force"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_a_file_as_the_download_directory() {
        let directory = temporary_path("not-directory");
        fs::write(&directory, b"file").unwrap();

        let error =
            download_to_directory(&directory, false, alpha_info(), |_, _| Ok(())).unwrap_err();

        assert!(matches!(
            error.kind(),
            io::ErrorKind::AlreadyExists | io::ErrorKind::InvalidInput
        ));
        fs::remove_file(directory).unwrap();
    }

    fn alpha_info() -> AlphaDownloadInfo {
        AlphaDownloadInfo {
            version: "Edgeless_Alpha_4.1.2".parse().unwrap(),
            name: "Edgeless_Alpha_4.1.2.wim".to_owned(),
        }
    }

    fn temporary_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-kernel-alpha-download-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
