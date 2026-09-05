//! 轻量级 HTTP 客户端封装。
//!
//! 目前只提供文本 GET 和文件下载，业务代码不得直接依赖具体 HTTP 库。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 请求 URL 并返回 UTF-8 文本响应。
pub fn get_text(url: &str) -> io::Result<String> {
    let response = get(url)?;
    response.into_string().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read HTTP response from {url}: {error}"),
        )
    })
}

/// 下载 URL 指向的内容到一个尚不存在的目标文件。
///
/// 下载过程先写入同目录的独占临时文件，完成后再移动到目标路径，避免读取方看到
/// 不完整的结果。目标文件已经存在时会拒绝覆盖；调用方应为并发下载分配不同目标。
pub fn download(url: &str, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "download destination already exists: {}",
                destination.display()
            ),
        ));
    }
    let response = get(url)?;
    let temporary = temporary_path(destination)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        io::copy(&mut response.into_reader(), &mut output)?;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary, destination)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn get(url: &str) -> io::Result<ureq::Response> {
    ureq::get(url).call().map_err(|error| match error {
        ureq::Error::Status(status, response) => io::Error::other(format!(
            "HTTP GET {url} failed with status {status}: {}",
            response.status_text()
        )),
        ureq::Error::Transport(error) => {
            io::Error::other(format!("HTTP GET {url} failed: {error}"))
        }
    })
}

fn temporary_path(destination: &Path) -> io::Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "download destination has no parent directory: {}",
                destination.display()
            ),
        )
    })?;
    let name = destination.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "download destination has no file name: {}",
                destination.display()
            ),
        )
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    Ok(parent.join(format!(
        ".{}.{}.{}.download",
        name.to_string_lossy(),
        std::process::id(),
        nonce
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_temporary_files_next_to_the_destination() {
        let destination = Path::new("downloads").join("kernel.iso");
        let temporary = temporary_path(&destination).unwrap();

        assert_eq!(temporary.parent(), Some(Path::new("downloads")));
        assert!(
            temporary
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".kernel.iso.")
        );
        assert!(
            temporary
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".download")
        );
    }
}
