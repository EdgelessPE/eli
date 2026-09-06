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
    response_into_text(response, url)
}

/// 请求带单个查询参数的 URL 并返回 UTF-8 文本响应。
///
/// 该入口适用于查询参数中包含邀请码等敏感信息的请求。错误信息只包含基础 URL，
/// 避免将查询参数写入终端或日志。
pub fn get_text_with_query_parameter(
    url: &str,
    parameter: &str,
    value: &str,
) -> io::Result<String> {
    let response = get_with_query_parameter(url, parameter, value)?;
    response_into_text(response, url)
}

fn response_into_text(response: ureq::Response, url: &str) -> io::Result<String> {
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
    download_reader(
        response.into_reader(),
        destination,
        false,
        |_| {},
        |_| Ok(()),
    )
}

/// 下载 URL 指向的内容，并在标准错误输出中实时显示进度条。
///
/// 服务器未提供内容长度时使用旋转指示器；下载仍会以已传输字节数更新。
pub fn download_with_progress(url: &str, destination: &Path, overwrite: bool) -> io::Result<()> {
    let response = get(url)?;
    download_response_with_progress(
        response,
        destination,
        overwrite,
        "正在下载 ISO",
        "ISO 下载失败",
        |_| Ok(()),
    )
}

/// 下载带单个查询参数的 URL 指向的内容，并显示进度条。
///
/// 该入口不会将查询参数的值包含在错误信息中，适用于邀请码等敏感参数。
pub fn download_with_progress_with_query_parameter(
    url: &str,
    parameter: &str,
    value: &str,
    destination: &Path,
    overwrite: bool,
) -> io::Result<()> {
    let response = get_with_query_parameter(url, parameter, value)?;
    download_response_with_progress(
        response,
        destination,
        overwrite,
        "正在下载文件",
        "文件下载失败",
        |_| Ok(()),
    )
}

/// 下载带单个查询参数的 URL，并在校验临时文件成功后才发布目标文件。
///
/// 进度和失败提示由业务调用方指定；校验失败时会删除临时文件，目标路径保持不存在。
pub fn download_with_progress_with_query_parameter_validated<F>(
    url: &str,
    parameter: &str,
    value: &str,
    destination: &Path,
    overwrite: bool,
    messages: (&str, &str),
    validate: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let response = get_with_query_parameter(url, parameter, value)?;
    download_response_with_progress(
        response,
        destination,
        overwrite,
        messages.0,
        messages.1,
        validate,
    )
}

fn download_response_with_progress<F>(
    response: ureq::Response,
    destination: &Path,
    overwrite: bool,
    progress_message: &str,
    failure_message: &str,
    validate: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let content_length = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    let progress = ProgressBar::new(content_length.unwrap_or(0));
    progress.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    progress.set_style(progress_style(content_length.is_some()));
    progress.set_message(progress_message.to_owned());
    progress.enable_steady_tick(std::time::Duration::from_millis(100));

    let result = download_reader(
        response.into_reader(),
        destination,
        overwrite,
        |bytes| {
            progress.inc(bytes);
        },
        validate,
    );
    if result.is_ok() {
        progress.finish_and_clear();
    } else {
        progress.abandon_with_message(failure_message.to_owned());
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

fn download_reader<R, P, V>(
    mut reader: R,
    destination: &Path,
    overwrite: bool,
    mut report_progress: P,
    validate: V,
) -> io::Result<()>
where
    R: Read,
    P: FnMut(u64),
    V: FnOnce(&Path) -> io::Result<()>,
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
        validate(&temporary)?;
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

fn get_with_query_parameter(url: &str, parameter: &str, value: &str) -> io::Result<ureq::Response> {
    ureq::get(url)
        .query(parameter, value)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(status, _) => {
                io::Error::other(format!("HTTP GET {url} failed with status {status}"))
            }
            ureq::Error::Transport(_) => {
                io::Error::other(format!("HTTP GET {url} failed: transport failure"))
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
    use std::io::{Cursor, Read, Write};
    use std::net::TcpListener;
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

        download_reader(
            Cursor::new(b"kernel data"),
            &destination,
            false,
            |bytes| {
                reported += bytes;
            },
            |_| Ok(()),
        )
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
                |_| Ok(()),
            )
        });
        started_receiver.recv().unwrap();

        let second_destination = destination.clone();
        let second = thread::spawn(move || {
            download_reader(
                Cursor::new(b"second"),
                &second_destination,
                false,
                |_| {},
                |_| Ok(()),
            )
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
    fn separate_processes_serialize_downloads_to_one_destination() {
        let directory = temporary_test_path("process-downloads");
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("kernel.wim");
        let first_result = directory.join("first.txt");
        let second_result = directory.join("second.txt");
        let mut first = download_child(&destination, &first_result);
        let mut second = download_child(&destination, &second_result);

        assert!(first.wait().unwrap().success());
        assert!(second.wait().unwrap().success());
        let mut results = vec![
            fs::read_to_string(first_result).unwrap(),
            fs::read_to_string(second_result).unwrap(),
        ];
        results.sort();
        assert_eq!(results, ["AlreadyExists", "Downloaded"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "仅由 separate_processes_serialize_downloads_to_one_destination 作为子进程调用"]
    fn download_process_child() {
        let Some(destination) = std::env::var_os("ELI_HTTP_TEST_DESTINATION") else {
            return;
        };
        let result_path = std::env::var_os("ELI_HTTP_TEST_RESULT").unwrap();
        let result = download_reader(
            Cursor::new(b"downloaded payload"),
            Path::new(&destination),
            false,
            |_| {},
            |_| {
                thread::sleep(Duration::from_millis(200));
                Ok(())
            },
        );
        let value = match result {
            Ok(()) => "Downloaded",
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => "AlreadyExists",
            Err(error) => panic!("unexpected child download error: {error}"),
        };
        fs::write(result_path, value).unwrap();
    }

    fn download_child(destination: &Path, result_path: &Path) -> std::process::Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "http::tests::download_process_child",
                "--ignored",
                "--nocapture",
            ])
            .env("ELI_HTTP_TEST_DESTINATION", destination)
            .env("ELI_HTTP_TEST_RESULT", result_path)
            .spawn()
            .unwrap()
    }

    #[test]
    fn force_overwrite_replaces_an_existing_download() {
        let destination = temporary_test_path("kernel.iso");
        fs::write(&destination, b"old").unwrap();

        download_reader(Cursor::new(b"new"), &destination, true, |_| {}, |_| Ok(())).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        fs::remove_file(&destination).unwrap();
        fs::remove_file(download_lock_path(&destination).unwrap()).unwrap();
    }

    #[test]
    fn validation_failure_does_not_publish_the_download() {
        let destination = temporary_test_path("invalid.wim");

        let error = download_reader(
            Cursor::new(b"not a WIM"),
            &destination,
            false,
            |_| {},
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid test payload",
                ))
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!destination.exists());
        fs::remove_file(download_lock_path(&destination).unwrap()).unwrap();
    }

    #[test]
    fn query_parameter_values_are_percent_encoded() {
        let (url, server) = one_request_server("200 OK", "ok");

        let response = get_text_with_query_parameter(&url, "token", "a&bc").unwrap();
        let request = server.join().unwrap();

        assert_eq!(response, "ok");
        assert!(request.starts_with("GET /?token=a%26bc HTTP/1.1\r\n"));
    }

    #[test]
    fn query_parameter_values_are_not_exposed_in_http_errors() {
        let (url, server) = one_request_server("403 Forbidden", "denied");
        let token = "secret-value";

        let error = get_text_with_query_parameter(&url, "token", token).unwrap_err();
        server.join().unwrap();

        assert!(!error.to_string().contains(token));
        assert!(error.to_string().contains("403"));
    }

    fn one_request_server(
        status: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 2048];
            let count = stream.read(&mut buffer).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(buffer[..count].to_vec()).unwrap()
        });
        (url, server)
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
