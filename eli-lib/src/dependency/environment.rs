use std::{fmt, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeEnvironment {
    WindowsNormal,
    WindowsPE,
    Linux,
    MacOS,
}

impl fmt::Display for RuntimeEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WindowsNormal => "WindowsNormal",
            Self::WindowsPE => "WindowsPE",
            Self::Linux => "Linux",
            Self::MacOS => "MacOS",
        })
    }
}

#[cfg(target_os = "windows")]
pub(super) fn detect() -> io::Result<RuntimeEnvironment> {
    use std::ptr;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };

    let path = "SYSTEM\\CurrentControlSet\\Control\\MiniNT"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut key: HKEY = ptr::null_mut();
    // 该注册表项由 Windows PE 创建，比判断固定盘符更可靠。
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) };
    if status == windows_sys::Win32::Foundation::ERROR_SUCCESS {
        unsafe {
            RegCloseKey(key);
        }
        Ok(RuntimeEnvironment::WindowsPE)
    } else if status == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        Ok(RuntimeEnvironment::WindowsNormal)
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(target_os = "linux")]
pub(super) fn detect() -> io::Result<RuntimeEnvironment> {
    Ok(RuntimeEnvironment::Linux)
}

#[cfg(target_os = "macos")]
pub(super) fn detect() -> io::Result<RuntimeEnvironment> {
    Ok(RuntimeEnvironment::MacOS)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
compile_error!("eli-lib supports only Windows, Linux, and macOS");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_current_operating_environment() {
        let environment = detect().unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(environment, RuntimeEnvironment::Linux);
        } else if cfg!(target_os = "macos") {
            assert_eq!(environment, RuntimeEnvironment::MacOS);
        } else {
            assert!(matches!(
                environment,
                RuntimeEnvironment::WindowsNormal | RuntimeEnvironment::WindowsPE
            ));
        }
    }
}
