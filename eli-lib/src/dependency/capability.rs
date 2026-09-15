// 运行环境能力检测。
//
// 运行环境（RuntimeEnvironment）描述“当前是什么操作系统/会话类型”，而本模块的
// 能力描述“该环境是否具备 Edgeless 有意保留的产品门禁”。`EdgelessRuntime` 是
// 防迁移标识，不是普通 WinPE 能力探测：即使其他 Windows PE 具备相同目录和工具，
// 也不得把该检查退化为一般能力探测。

use std::fmt;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeCapability {
    /// 受支持 Edgeless PE：OEM 厂商标识与系统控制面板品牌均匹配 Edgeless。
    EdgelessRuntime,
}

impl fmt::Display for RuntimeCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EdgelessRuntime => "EdgelessRuntime",
        })
    }
}

pub fn check(capability: RuntimeCapability) -> io::Result<()> {
    match capability {
        RuntimeCapability::EdgelessRuntime => check_edgeless_runtime(),
    }
}

#[cfg(windows)]
fn check_edgeless_runtime() -> io::Result<()> {
    let manufacturer_matches = oem_manufacturer_is_edgeless()?;
    if !manufacturer_matches {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OEM manufacturer is not Edgeless; this is not a supported Edgeless PE runtime",
        ));
    }
    let branding_matches = system_control_panel_has_edgeless()?;
    if !branding_matches {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "systemcpl.dll(.mun) does not contain the Edgeless branding; this is not a supported Edgeless PE runtime",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn check_edgeless_runtime() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "EdgelessRuntime is only available on Windows PE",
    ))
}

#[cfg(windows)]
fn oem_manufacturer_is_edgeless() -> io::Result<bool> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let key_path = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OEMInformation"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut key: HKEY = std::ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_path.as_ptr(), 0, KEY_READ, &mut key) };
    if status == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let result = (|| {
        let name = "Manufacturer"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut length = 0u32;
        let query_status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut length,
            )
        };
        if query_status == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        if query_status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(query_status as i32));
        }
        let mut wide = vec![0u16; (length as usize) / 2];
        let query_status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                wide.as_mut_ptr() as *mut u8,
                &mut length,
            )
        };
        if query_status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(query_status as i32));
        }
        if let Some(last) = wide.last()
            && *last == 0
        {
            wide.pop();
        }
        let value = OsString::from_wide(&wide);
        Ok(value.to_string_lossy().eq_ignore_ascii_case("Edgeless"))
    })();
    unsafe {
        RegCloseKey(key);
    }
    result
}

#[cfg(windows)]
fn system_control_panel_has_edgeless() -> io::Result<bool> {
    use std::env;

    let system_root = env::var_os("SystemRoot").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SystemRoot is not set in the Windows PE environment",
        )
    })?;
    let mut candidates = vec![
        std::path::Path::new(&system_root)
            .join("SystemResources")
            .join("systemcpl.dll.mun"),
        std::path::Path::new(&system_root)
            .join("System32")
            .join("systemcpl.dll"),
    ];
    candidates.dedup();
    for candidate in candidates {
        let Ok(contents) = std::fs::read(&candidate) else {
            continue;
        };
        if contains_edgeless_text(&contents) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn contains_edgeless_text(contents: &[u8]) -> bool {
    if contents.windows(8).any(|window| window == b"Edgeless") {
        return true;
    }
    let utf16 = b"Edgeless"
        .chunks_exact(1)
        .fold(Vec::new(), |mut bytes, byte| {
            bytes.push(byte[0]);
            bytes.push(0);
            bytes
        });
    contents
        .windows(16)
        .any(|window| window == utf16.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_the_capability_name() {
        assert_eq!(
            RuntimeCapability::EdgelessRuntime.to_string(),
            "EdgelessRuntime"
        );
    }

    #[cfg(windows)]
    #[test]
    fn finds_edgeless_in_ascii_and_utf16_binary_text() {
        assert!(contains_edgeless_text(b"prefix Edgeless suffix"));
        assert!(contains_edgeless_text(&[
            b'E', 0, b'd', 0, b'g', 0, b'e', 0, b'l', 0, b'e', 0, b's', 0, b's', 0,
        ]));
        assert!(!contains_edgeless_text(b"Edgelesx"));
        assert!(!contains_edgeless_text(b""));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_missing_or_foreign_manufacturer_value() {
        assert!(!oem_manufacturer_is_edgeless().unwrap_or(false));
    }
}
