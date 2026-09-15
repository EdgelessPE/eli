// Windows 平台 Shell Link COM 实现。
//
// windows-sys 不提供 COM 接口虚表，这里按 SDK 固定布局手写 IShellLinkW /
// IPersistFile 虚表。只调用 IPersistFile::Load、IShellLinkW::SetIconLocation
// 和 IPersistFile::Save；Save 成功后不再重复 Load 验证。

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::core::{GUID, PCWSTR, PWSTR};

const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
const COINIT_APARTMENTTHREADED: u32 = 2;
const CLSCTX_INPROC_SERVER: u32 = 1;
const STGM_READWRITE: u32 = 2;
#[cfg(test)]
const STGM_READ: u32 = 0;

type HResult = i32;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const GUID,
        *mut *mut core::ffi::c_void,
    ) -> HResult,
    add_ref: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
}

#[repr(C)]
struct IPersistVtbl {
    unknown: IUnknownVtbl,
    get_class_id: unsafe extern "system" fn(*mut core::ffi::c_void, *mut GUID) -> HResult,
}

#[repr(C)]
struct IPersistFileVtbl {
    persist: IPersistVtbl,
    is_dirty: unsafe extern "system" fn(*mut core::ffi::c_void) -> HResult,
    load: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, u32) -> HResult,
    save: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HResult,
    save_completed: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR) -> HResult,
    get_cur_file: unsafe extern "system" fn(*mut core::ffi::c_void, *mut PWSTR) -> HResult,
}

#[repr(C)]
struct IShellLinkWVtbl {
    unknown: IUnknownVtbl,
    get_path: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        PWSTR,
        u32,
        *mut core::ffi::c_void,
        u32,
    ) -> HResult,
    get_id_list:
        unsafe extern "system" fn(*mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HResult,
    set_id_list:
        unsafe extern "system" fn(*mut core::ffi::c_void, *const core::ffi::c_void) -> HResult,
    get_description: unsafe extern "system" fn(*mut core::ffi::c_void, PWSTR, u32) -> HResult,
    set_description: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR) -> HResult,
    get_working_directory: unsafe extern "system" fn(*mut core::ffi::c_void, PWSTR, u32) -> HResult,
    set_working_directory: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR) -> HResult,
    get_arguments: unsafe extern "system" fn(*mut core::ffi::c_void, PWSTR, u32) -> HResult,
    set_arguments: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR) -> HResult,
    get_hotkey: unsafe extern "system" fn(*mut core::ffi::c_void, *mut u16) -> HResult,
    set_hotkey: unsafe extern "system" fn(*mut core::ffi::c_void, u16) -> HResult,
    get_show_cmd: unsafe extern "system" fn(*mut core::ffi::c_void, *mut u32) -> HResult,
    set_show_cmd: unsafe extern "system" fn(*mut core::ffi::c_void, u32) -> HResult,
    get_icon_location:
        unsafe extern "system" fn(*mut core::ffi::c_void, PWSTR, u32, *mut i32) -> HResult,
    set_icon_location: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HResult,
    set_relative_path: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, u32) -> HResult,
    resolve: unsafe extern "system" fn(*mut core::ffi::c_void, isize, u32) -> HResult,
    set_path: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR) -> HResult,
}

struct ComApartment;

impl ComApartment {
    fn initialize_sta() -> io::Result<Self> {
        let hr = unsafe {
            windows_sys::Win32::System::Com::CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED)
        };
        if hr < 0 {
            Err(hresult_error(
                "CoInitializeEx(COINIT_APARTMENTTHREADED)",
                hr,
            ))
        } else {
            Ok(Self)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Com::CoUninitialize();
        }
    }
}

fn hresult_error(operation: &str, hr: HResult) -> io::Error {
    io::Error::other(format!("{operation} failed with HRESULT 0x{hr:08x}"))
}

unsafe fn vtable<T>(interface: *mut core::ffi::c_void) -> &'static T {
    unsafe { &**(interface as *mut *const T) }
}

pub(super) fn set_icon_location(link: &Path, icon: &Path) -> io::Result<()> {
    let _apartment = ComApartment::initialize_sta()?;

    let mut shell_link: *mut core::ffi::c_void = ptr::null_mut();
    let hr = unsafe {
        windows_sys::Win32::System::Com::CoCreateInstance(
            &CLSID_SHELL_LINK,
            ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ISHELL_LINK_W,
            &mut shell_link,
        )
    };
    if hr < 0 {
        return Err(hresult_error("CoCreateInstance(ShellLink)", hr));
    }

    let shell_vtable = unsafe { vtable::<IShellLinkWVtbl>(shell_link) };
    let mut persist_file: *mut core::ffi::c_void = ptr::null_mut();
    let query_hr = unsafe {
        (shell_vtable.unknown.query_interface)(shell_link, &IID_IPERSIST_FILE, &mut persist_file)
    };
    if query_hr < 0 {
        unsafe {
            (shell_vtable.unknown.release)(shell_link);
        }
        return Err(hresult_error("QueryInterface(IPersistFile)", query_hr));
    }

    let result = (|| {
        let link_wide = link
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let icon_wide = icon
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();

        let persist_vtable = unsafe { vtable::<IPersistFileVtbl>(persist_file) };
        let hr = unsafe { (persist_vtable.load)(persist_file, link_wide.as_ptr(), STGM_READWRITE) };
        if hr < 0 {
            return Err(hresult_error(
                &format!("IPersistFile::Load for {}", link.display()),
                hr,
            ));
        }
        let hr = unsafe { (shell_vtable.set_icon_location)(shell_link, icon_wide.as_ptr(), 0) };
        if hr < 0 {
            return Err(hresult_error(
                &format!("IShellLinkW::SetIconLocation for {}", icon.display()),
                hr,
            ));
        }
        let hr = unsafe { (persist_vtable.save)(persist_file, link_wide.as_ptr(), 1) };
        if hr < 0 {
            return Err(hresult_error(
                &format!("IPersistFile::Save for {}", link.display()),
                hr,
            ));
        }
        Ok(())
    })();

    unsafe {
        let persist_vtable = vtable::<IPersistFileVtbl>(persist_file);
        (persist_vtable.persist.unknown.release)(persist_file);
        (shell_vtable.unknown.release)(shell_link);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eli-shell-link-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn create_shell_link() -> (*mut core::ffi::c_void, *mut core::ffi::c_void) {
        let mut shell_link = ptr::null_mut();
        let hr = unsafe {
            windows_sys::Win32::System::Com::CoCreateInstance(
                &CLSID_SHELL_LINK,
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_ISHELL_LINK_W,
                &mut shell_link,
            )
        };
        assert!(hr >= 0, "CoCreateInstance failed: 0x{hr:08x}");
        let shell_vtable = unsafe { vtable::<IShellLinkWVtbl>(shell_link) };
        let mut persist_file = ptr::null_mut();
        let hr = unsafe {
            (shell_vtable.unknown.query_interface)(
                shell_link,
                &IID_IPERSIST_FILE,
                &mut persist_file,
            )
        };
        assert!(hr >= 0, "QueryInterface failed: 0x{hr:08x}");
        (shell_link, persist_file)
    }

    unsafe fn release_interfaces(
        shell_link: *mut core::ffi::c_void,
        persist_file: *mut core::ffi::c_void,
    ) {
        let persist_vtable = unsafe { vtable::<IPersistFileVtbl>(persist_file) };
        let shell_vtable = unsafe { vtable::<IShellLinkWVtbl>(shell_link) };
        unsafe {
            (persist_vtable.persist.unknown.release)(persist_file);
            (shell_vtable.unknown.release)(shell_link);
        }
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn decode_wide(buffer: &[u16]) -> OsString {
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        OsString::from_wide(&buffer[..length])
    }

    #[test]
    fn changes_only_the_icon_location_of_a_real_shortcut() {
        let root = test_root();
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target.exe");
        let icon = root.join("theme icon.ico");
        let link = root.join("Tool.lnk");
        std::fs::write(&target, b"target").unwrap();
        std::fs::write(&icon, b"icon").unwrap();

        {
            let _apartment = ComApartment::initialize_sta().unwrap();
            let (shell_link, persist_file) = create_shell_link();
            let shell_vtable = unsafe { vtable::<IShellLinkWVtbl>(shell_link) };
            let persist_vtable = unsafe { vtable::<IPersistFileVtbl>(persist_file) };
            let target_wide = wide_path(&target);
            let link_wide = wide_path(&link);
            let hr = unsafe { (shell_vtable.set_path)(shell_link, target_wide.as_ptr()) };
            assert!(hr >= 0, "IShellLinkW::SetPath failed: 0x{hr:08x}");
            let hr = unsafe { (persist_vtable.save)(persist_file, link_wide.as_ptr(), 1) };
            assert!(hr >= 0, "IPersistFile::Save failed: 0x{hr:08x}");
            unsafe {
                release_interfaces(shell_link, persist_file);
            }
        }

        set_icon_location(&link, &icon).unwrap();

        {
            let _apartment = ComApartment::initialize_sta().unwrap();
            let (shell_link, persist_file) = create_shell_link();
            let shell_vtable = unsafe { vtable::<IShellLinkWVtbl>(shell_link) };
            let persist_vtable = unsafe { vtable::<IPersistFileVtbl>(persist_file) };
            let link_wide = wide_path(&link);
            let hr = unsafe { (persist_vtable.load)(persist_file, link_wide.as_ptr(), STGM_READ) };
            assert!(hr >= 0, "IPersistFile::Load failed: 0x{hr:08x}");

            let mut icon_buffer = vec![0u16; 32_768];
            let mut icon_index = -1;
            let hr = unsafe {
                (shell_vtable.get_icon_location)(
                    shell_link,
                    icon_buffer.as_mut_ptr(),
                    icon_buffer.len() as u32,
                    &mut icon_index,
                )
            };
            assert!(hr >= 0, "IShellLinkW::GetIconLocation failed: 0x{hr:08x}");
            assert_eq!(std::path::PathBuf::from(decode_wide(&icon_buffer)), icon);
            assert_eq!(icon_index, 0);

            let mut target_buffer = vec![0u16; 32_768];
            let hr = unsafe {
                (shell_vtable.get_path)(
                    shell_link,
                    target_buffer.as_mut_ptr(),
                    target_buffer.len() as u32,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(hr >= 0, "IShellLinkW::GetPath failed: 0x{hr:08x}");
            assert_eq!(
                std::path::PathBuf::from(decode_wide(&target_buffer)),
                target
            );
            unsafe {
                release_interfaces(shell_link, persist_file);
            }
        }

        std::fs::remove_dir_all(root).unwrap();
    }
}
