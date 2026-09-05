use crate::api::edgeless;
use crate::http;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 内核下载命令的执行结果。
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadResult {
    /// 已完成 ISO 下载。
    Downloaded(PathBuf),
    /// 目标文件已存在，因此未发起下载。
    Skipped(PathBuf),
}

/// 下载最新 Edgeless ISO 到调用方指定的目录，并返回执行结果。
///
/// 同一目标文件的重复下载会被拒绝覆盖。下载内容先保存为同目录临时文件，成功后
/// 才发布为最终 ISO，避免并发读取者看到不完整文件。
pub fn download(directory: &Path, force: bool) -> io::Result<DownloadResult> {
    let info = edgeless::latest_iso_download_info()?;
    let file_name = iso_file_name(&info.name)?;
    fs::create_dir_all(directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to create download directory {}: {error}",
                directory.display()
            ),
        )
    })?;
    if !directory.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "download directory is not a directory: {}",
                directory.display()
            ),
        ));
    }
    let destination = directory.join(file_name);
    if let Some(result) = skip_existing_file(&destination, force) {
        return Ok(result);
    }
    http::download_with_progress(&info.url, &destination, force)?;
    Ok(DownloadResult::Downloaded(destination))
}

fn skip_existing_file(destination: &Path, force: bool) -> Option<DownloadResult> {
    if destination.exists() && !force {
        return Some(DownloadResult::Skipped(destination.to_owned()));
    }
    None
}

fn iso_file_name(name: &str) -> io::Result<&str> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless ISO info API returned an invalid file name: {name:?}"),
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn accepts_the_api_iso_file_name() {
        assert_eq!(
            iso_file_name("Edgeless_Beta.iso").unwrap(),
            "Edgeless_Beta.iso"
        );
    }

    #[test]
    fn rejects_an_unsafe_api_iso_file_name() {
        let error = iso_file_name("../Edgeless.iso").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn skips_the_file_named_by_the_api_when_it_already_exists() {
        let directory = temporary_directory();
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("Edgeless_Beta_From_API.iso");
        fs::write(&destination, b"existing ISO").unwrap();

        assert_eq!(
            skip_existing_file(&destination, false),
            Some(DownloadResult::Skipped(destination.clone()))
        );
        assert_eq!(skip_existing_file(&destination, true), None);
        fs::remove_dir_all(directory).unwrap();
    }

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-kernel-download-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
