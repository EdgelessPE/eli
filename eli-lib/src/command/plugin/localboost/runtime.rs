#![cfg_attr(not(windows), allow(dead_code))]

use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::fs;

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Gbk,
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct CompatibleText {
    pub(crate) contents: String,
    encoding: TextEncoding,
}

#[cfg(windows)]
impl CompatibleText {
    pub(crate) fn decode(bytes: &[u8]) -> io::Result<Self> {
        if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
            return Ok(Self {
                contents: String::from_utf8(bytes.to_vec()).map_err(invalid_text)?,
                encoding: TextEncoding::Utf8Bom,
            });
        }
        if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
            return Ok(Self {
                contents: decode_utf16(bytes, true)?,
                encoding: TextEncoding::Utf16Le,
            });
        }
        if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
            return Ok(Self {
                contents: decode_utf16(bytes, false)?,
                encoding: TextEncoding::Utf16Be,
            });
        }
        match String::from_utf8(bytes.to_vec()) {
            Ok(contents) => Ok(Self {
                contents,
                encoding: TextEncoding::Utf8,
            }),
            Err(_) => Ok(Self {
                contents: decode_code_page(bytes, 936)?,
                encoding: TextEncoding::Gbk,
            }),
        }
    }

    pub(crate) fn encode(&self, contents: &str) -> io::Result<Vec<u8>> {
        match self.encoding {
            TextEncoding::Utf8 => Ok(contents.as_bytes().to_vec()),
            TextEncoding::Utf8Bom => {
                let mut bytes = vec![0xef, 0xbb, 0xbf];
                bytes.extend_from_slice(contents.as_bytes());
                Ok(bytes)
            }
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let little_endian = self.encoding == TextEncoding::Utf16Le;
                let mut bytes = if little_endian {
                    vec![0xff, 0xfe]
                } else {
                    vec![0xfe, 0xff]
                };
                for value in contents.encode_utf16() {
                    let encoded = if little_endian {
                        value.to_le_bytes()
                    } else {
                        value.to_be_bytes()
                    };
                    bytes.extend_from_slice(&encoded);
                }
                Ok(bytes)
            }
            TextEncoding::Gbk => encode_code_page(contents, 936).or_else(|_| {
                let mut bytes = vec![0xef, 0xbb, 0xbf];
                bytes.extend_from_slice(contents.as_bytes());
                Ok(bytes)
            }),
        }
    }

    pub(crate) fn newline(&self) -> &'static str {
        if self.contents.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }
}

#[cfg(windows)]
pub(crate) fn read_compatible_text(path: &Path) -> io::Result<Option<CompatibleText>> {
    match fs::read(path) {
        Ok(bytes) => CompatibleText::decode(&bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn invalid_text(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(windows)]
fn decode_utf16(bytes: &[u8], little_endian: bool) -> io::Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(invalid_text("UTF-16 text has an odd byte length"));
    }
    let values = bytes.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    String::from_utf16(&values.collect::<Vec<_>>()).map_err(invalid_text)
}

#[cfg(windows)]
fn decode_code_page(bytes: &[u8], code_page: u32) -> io::Result<String> {
    use std::ptr;
    use windows_sys::Win32::Globalization::MultiByteToWideChar;

    if bytes.is_empty() {
        return Ok(String::new());
    }
    let byte_count = i32::try_from(bytes.len()).map_err(invalid_text)?;
    // 安全性：两次调用使用同一有效输入缓冲区，第二次输出缓冲区按首调用结果分配。
    let wide_count = unsafe {
        MultiByteToWideChar(code_page, 0, bytes.as_ptr(), byte_count, ptr::null_mut(), 0)
    };
    if wide_count == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut wide = vec![0_u16; wide_count as usize];
    // 安全性：输出缓冲区容量等于系统报告的所需 UTF-16 单元数。
    let converted = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            byte_count,
            wide.as_mut_ptr(),
            wide_count,
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    String::from_utf16(&wide).map_err(invalid_text)
}

#[cfg(windows)]
fn encode_code_page(contents: &str, code_page: u32) -> io::Result<Vec<u8>> {
    use std::ptr;
    use windows_sys::Win32::Globalization::WideCharToMultiByte;

    if contents.is_empty() {
        return Ok(Vec::new());
    }
    let wide = contents.encode_utf16().collect::<Vec<_>>();
    let wide_count = i32::try_from(wide.len()).map_err(invalid_text)?;
    let mut used_default = 0;
    // 安全性：输入为有效 UTF-16，输出为空用于查询所需字节数。
    let byte_count = unsafe {
        WideCharToMultiByte(
            code_page,
            0x0000_0400,
            wide.as_ptr(),
            wide_count,
            ptr::null_mut(),
            0,
            ptr::null(),
            &mut used_default,
        )
    };
    if byte_count == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = vec![0_u8; byte_count as usize];
    used_default = 0;
    // 安全性：输出缓冲区容量等于系统报告的所需字节数。
    let converted = unsafe {
        WideCharToMultiByte(
            code_page,
            0x0000_0400,
            wide.as_ptr(),
            wide_count,
            bytes.as_mut_ptr(),
            byte_count,
            ptr::null(),
            &mut used_default,
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    if used_default != 0 {
        return Err(invalid_text(format!(
            "text cannot be represented by Windows code page {code_page}"
        )));
    }
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimePaths {
    pub(crate) edgeless: PathBuf,
    pub(crate) installers: PathBuf,
    pub(crate) system_drive: PathBuf,
    pub(crate) plugin_info: PathBuf,
    pub(crate) local_boost: PathBuf,
    pub(crate) selection_file: PathBuf,
    pub(crate) loaded: PathBuf,
}

impl RuntimePaths {
    #[cfg(windows)]
    pub(crate) fn detect() -> io::Result<Self> {
        let program_files = env::var_os("ProgramFiles").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "ProgramFiles is not set in the Windows PE environment",
            )
        })?;
        let system_drive = env::var_os("SystemDrive").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "SystemDrive is not set in the Windows PE environment",
            )
        })?;
        let system_drive = drive_root(Path::new(&system_drive));
        let users = system_drive.join("Users");
        let local_boost = users.join("LocalBoost");
        let edgeless = PathBuf::from(program_files).join("Edgeless");
        Ok(Self {
            installers: edgeless.join("安装程序"),
            edgeless,
            system_drive,
            plugin_info: users.join("Plugins_info"),
            selection_file: local_boost.join("repoPart.txt"),
            loaded: local_boost.join("Loaded"),
            local_boost,
        })
    }
}

pub(crate) fn safe_component(value: &OsStr) -> bool {
    if value.is_empty() {
        return false;
    }
    let text = value.to_string_lossy();
    if text == "."
        || text == ".."
        || text.ends_with(['.', ' '])
        || text.contains(['/', '\\', ':'])
        || text.chars().any(|character| character < ' ')
    {
        return false;
    }
    let device_stem = text
        .split_once('.')
        .map_or(text.as_ref(), |(stem, _)| stem)
        .to_ascii_uppercase();
    if matches!(
        device_stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$"
    ) || device_stem
        .strip_prefix("COM")
        .or_else(|| device_stem.strip_prefix("LPT"))
        .is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
    {
        return false;
    }
    let path = Path::new(value);
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(windows)]
pub(crate) fn loaded_marker(paths: &RuntimePaths, plugin_name: &OsStr) -> PathBuf {
    let mut marker = paths.loaded.join(plugin_name).into_os_string();
    marker.push(".loaded");
    PathBuf::from(marker)
}

#[cfg(windows)]
pub(crate) fn is_loaded(paths: &RuntimePaths, plugin_name: &OsStr) -> bool {
    loaded_marker(paths, plugin_name).is_file()
}

#[cfg(windows)]
pub(crate) fn commit_loaded(paths: &RuntimePaths, plugin_name: &OsStr) -> io::Result<()> {
    use super::super::load::replace_file;

    fs::create_dir_all(&paths.loaded)?;
    replace_file(
        &loaded_marker(paths, plugin_name),
        plugin_name.to_string_lossy().as_bytes(),
    )
}

#[cfg(windows)]
pub(crate) fn clear_loaded(paths: &RuntimePaths, plugin_name: &OsStr) -> io::Result<()> {
    match fs::remove_file(loaded_marker(paths, plugin_name)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub(crate) fn drive_root(path: &Path) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    let mut characters = value.chars();
    match (characters.next(), characters.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            PathBuf::from(format!(r"{}:\", letter.to_ascii_uppercase()))
        }
        _ => path.to_owned(),
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct LocalBoostLock {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for LocalBoostLock {}
#[cfg(windows)]
unsafe impl Sync for LocalBoostLock {}

#[cfg(windows)]
impl LocalBoostLock {
    pub(crate) fn new() -> io::Result<Self> {
        use std::ptr;
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = "Local\\Edgeless.eli.plugin-localboost.lifecycle"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // 安全性：名称缓冲区以 NUL 结尾，返回句柄由 Drop 关闭。
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }

    pub(crate) fn acquire(&self) -> io::Result<LocalBoostGuard<'_>> {
        use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

        // 安全性：句柄在 self 生命周期内有效，等待不会转移所有权。
        match unsafe { WaitForSingleObject(self.handle, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(LocalBoostGuard { lock: self }),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

#[cfg(windows)]
impl Drop for LocalBoostLock {
    fn drop(&mut self) {
        // 安全性：句柄由本实例持有且只在此处关闭一次。
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
pub(crate) struct LocalBoostGuard<'a> {
    lock: &'a LocalBoostLock,
}

#[cfg(windows)]
impl Drop for LocalBoostGuard<'_> {
    fn drop(&mut self) {
        // 安全性：当前线程持有该互斥锁，释放后句柄仍由 LocalBoostLock 管理。
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.lock.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn accepts_a_unicode_safe_component() {
        assert!(safe_component(OsStr::new("工具箱_1.0_作者")));
    }

    #[test]
    fn rejects_unsafe_components() {
        for value in [
            "",
            ".",
            "..",
            "a/b",
            r"a\b",
            r"C:\plugin",
            "name. ",
            "CON",
            "LPT1.txt",
        ] {
            assert!(!safe_component(OsStr::new(value)), "{value}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn loaded_markers_preserve_dots_in_plugin_names() {
        let paths = RuntimePaths {
            edgeless: PathBuf::new(),
            installers: PathBuf::new(),
            system_drive: PathBuf::new(),
            plugin_info: PathBuf::new(),
            local_boost: PathBuf::new(),
            selection_file: PathBuf::new(),
            loaded: PathBuf::from("Loaded"),
        };

        assert_eq!(
            loaded_marker(&paths, OsStr::new("tool.preview")),
            PathBuf::from("Loaded/tool.preview.loaded")
        );
    }

    #[cfg(windows)]
    #[test]
    fn compatible_text_preserves_legacy_gbk() {
        let bytes = b"plugin\xa3\xa8bot\xa3\xa9\r\n";
        let text = CompatibleText::decode(bytes).unwrap();

        assert_eq!(text.contents, "plugin（bot）\r\n");
        assert_eq!(text.encode(&text.contents).unwrap(), bytes);
    }

    #[cfg(windows)]
    #[test]
    fn lifecycle_lock_serializes_threads() {
        use std::sync::{Arc, mpsc};
        use std::time::Duration;

        let lock = Arc::new(LocalBoostLock::new().unwrap());
        let guard = lock.acquire().unwrap();
        let worker_lock = Arc::clone(&lock);
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = worker_lock.acquire().unwrap();
            sender.send(()).unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
        drop(guard);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
