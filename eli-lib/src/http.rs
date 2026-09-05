//! 轻量级 HTTP 客户端封装。
//!
//! 目前只提供文本 GET 和文件下载，业务代码不得直接依赖具体 HTTP 库。

use fs2::FileExt;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
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
/// 不完整的结果。相同目标的并发下载会由同目录锁串行化，后执行者会因目标文件
/// 已存在而拒绝覆盖。
pub fn download(url: &str, destination: &Path) -> io::Result<()> {
    let response = get(url)?;
    download_reader(response.into_reader(), destination, false, |_| {})
}

/// 下载 URL 指向的内容，并在标准错误输出中实时显示进度条。
///
/// 服务器未提供内容长度时使用旋转指示器；下载仍会以已传输字节数更新。
pub fn download_with_progress(url: &str, destination: &Path, overwrite: bool) -> io::Result<()> {
    let response = get(url)?;
    let content_length = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    let progress = ProgressBar::new(content_length.unwrap_or(0));
    progress.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    progress.set_style(progress_style(content_length.is_some()));
    progress.set_message("正在下载 ISO");
    progress.enable_steady_tick(std::time::Duration::from_millis(100));

    let result = download_reader(response.into_reader(), destination, overwrite, |bytes| {
        progress.inc(bytes);
    });
    if result.is_ok() {
        progress.finish_and_clear();
    } else {
        progress.abandon_with_message("ISO 下载失败");
    }
    result
}

fn progress_style(has_content_length: bool) -> ProgressStyle {
    let template = if has_content_length {
        "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, {eta}) {msg}"
    } else {
        "{spinner:.green} [{elapsed_precise}] {bytes} ({bytes_per_sec}) {msg}"
    };
    ProgressStyle::with_template(template)
        .expect("进度条模板固定有效")
        .progress_chars("#>-")
}

fn download_reader<R, F>(
    mut reader: R,
    destination: &Path,
    overwrite: bool,
    mut report_progress: F,
) -> io::Result<()>
where
    R: Read,
    F: FnMut(u64),
{
    let _lock = acquire_download_lock(destination)?;
    if destination.exists() && !overwrite {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "download destination already exists: {}",
                destination.display()
            ),
        ));
    }
    let temporary = temporary_path(destination)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            report_progress(count as u64);
        }
        output.flush()?;
        output.sync_all()?;
        drop(output);
        publish_temporary_file(&temporary, destination, overwrite)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn publish_temporary_file(temporary: &Path, destination: &Path, overwrite: bool) -> io::Result<()> {
    if !destination.exists() {
        return fs::rename(temporary, destination);
    }
    if !overwrite {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "download destination already exists: {}",
                destination.display()
            ),
        ));
    }

    let backup = backup_path(destination)?;
    fs::rename(destination, &backup)?;
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let restore_result = fs::rename(&backup, destination);
            if let Err(restore_error) = restore_result {
                return Err(io::Error::other(format!(
                    "failed to publish download {}: {error}; failed to restore original file: {restore_error}",
                    destination.display()
                )));
            }
            Err(error)
        }
    }
}

fn acquire_download_lock(destination: &Path) -> io::Result<File> {
    let lock_path = download_lock_path(destination)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.lock_exclusive().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to lock download destination {}: {error}",
                destination.display()
            ),
        )
    })?;
    Ok(lock)
}

fn download_lock_path(destination: &Path) -> io::Result<PathBuf> {
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
    Ok(parent.join(format!(".{}.download.lock", name.to_string_lossy())))
}

fn backup_path(destination: &Path) -> io::Result<PathBuf> {
    let temporary = temporary_path(destination)?;
    Ok(temporary.with_extension("backup"))
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
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn reports_downloaded_bytes_while_writing_the_file() {
        let destination = temporary_test_path("kernel.iso");
        let mut reported = 0;

        download_reader(Cursor::new(b"kernel data"), &destination, false, |bytes| {
            reported += bytes;
        })
        .unwrap();

        assert_eq!(reported, 11);
        assert_eq!(fs::read(&destination).unwrap(), b"kernel data");
        fs::remove_file(&destination).unwrap();
        fs::remove_file(download_lock_path(&destination).unwrap()).unwrap();
    }

    #[test]
    fn concurrent_downloads_to_one_destination_do_not_overwrite_each_other() {
        let destination = temporary_test_path("kernel.iso");
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let first_destination = destination.clone();
        let first = thread::spawn(move || {
            download_reader(
                BlockingReader {
                    started_sender,
                    release_receiver,
                    remaining: Some(b"first"),
                },
                &first_destination,
                false,
                |_| {},
            )
        });
        started_receiver.recv().unwrap();

        let second_destination = destination.clone();
        let second = thread::spawn(move || {
            download_reader(Cursor::new(b"second"), &second_destination, false, |_| {})
        });
        release_sender.send(()).unwrap();

        first.join().unwrap().unwrap();
        let error = second.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&destination).unwrap(), b"first");
        fs::remove_file(&destination).unwrap();
        fs::remove_file(download_lock_path(&destination).unwrap()).unwrap();
    }

    #[test]
    fn force_overwrite_replaces_an_existing_download() {
        let destination = temporary_test_path("kernel.iso");
        fs::write(&destination, b"old").unwrap();

        download_reader(Cursor::new(b"new"), &destination, true, |_| {}).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        fs::remove_file(&destination).unwrap();
        fs::remove_file(download_lock_path(&destination).unwrap()).unwrap();
    }

    struct BlockingReader {
        started_sender: mpsc::Sender<()>,
        release_receiver: mpsc::Receiver<()>,
        remaining: Option<&'static [u8]>,
    }

    impl Read for BlockingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let Some(remaining) = self.remaining.take() else {
                return Ok(0);
            };
            self.started_sender.send(()).unwrap();
            self.release_receiver.recv().unwrap();
            buffer[..remaining.len()].copy_from_slice(remaining);
            Ok(remaining.len())
        }
    }

    fn temporary_test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-http-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
