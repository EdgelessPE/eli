// Windows 平台后端：承载 theme apply 的真实 Win32 副作用。
//
// 7-Zip/PECMD 通过依赖管理模块从 PATH 解析并探测；注册表写入逐次检查返回值并
// 关闭句柄；光标刷新使用 SPI_SETCURSORS；Explorer 生命周期只操作当前会话、
// 当前用户拥有的窗口进程；ESS 的 ACL 提升只在 AccessDenied 时临时进行并最终恢复。
// 非 Windows 平台保留统一接口，由环境检查阶段返回明确的 WindowsPE 不支持错误。

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

use crate::Ctx;
use crate::dependency::ProgramDependency;

use super::archive::{ArchiveEntry, parse_listing};
use super::ems::{BASE_SLOTS, OPTIONAL_SLOTS, scheme_field_string};
use super::transaction::publish_directory_atomically;
use super::{CursorSnapshot, RegistryValueSnapshot, ThemeBackend, ThemePaths};

/// 主题提交全局 mutex（本地会话）。等待采用有界超时；超时返回 WouldBlock。
const THEME_APPLY_MUTEX_NAME: &str = "Local\\Edgeless.Eli.ThemeApply";
const THEME_APPLY_WAIT_MILLIS: u32 = 60_000;
const SHELL_WAIT_MILLIS: u32 = 60_000;
const SHELL_AUTO_RESTART_GRACE_MILLIS: u32 = 5_000;

pub struct WindowsThemeBackend<'a> {
    ctx: &'a Ctx,
    paths: OnceLock<ThemePaths>,
}

impl<'a> WindowsThemeBackend<'a> {
    pub fn new(ctx: &'a Ctx) -> io::Result<Self> {
        Ok(Self {
            ctx,
            paths: OnceLock::new(),
        })
    }

    fn paths(&self) -> io::Result<&ThemePaths> {
        if let Some(paths) = self.paths.get() {
            return Ok(paths);
        }
        let paths = resolve_theme_paths()?;
        let _ = self.paths.set(paths);
        Ok(self
            .paths
            .get()
            .expect("theme paths were initialized by this backend"))
    }

    fn seven_zip(&self) -> io::Result<PathBuf> {
        let programs = self
            .ctx
            .dependencies()
            .require_programs(&[ProgramDependency::SevenZip])?;
        programs
            .executable(ProgramDependency::SevenZip)
            .map(Path::to_owned)
    }

    fn pecmd(&self) -> io::Result<PathBuf> {
        let programs = self
            .ctx
            .dependencies()
            .require_programs(&[ProgramDependency::Pecmd])?;
        programs
            .executable(ProgramDependency::Pecmd)
            .map(Path::to_owned)
    }
}

impl ThemeBackend for WindowsThemeBackend<'_> {
    fn theme_paths(&self) -> io::Result<ThemePaths> {
        Ok(self.paths()?.clone())
    }

    fn require_seven_zip(&self) -> io::Result<()> {
        self.seven_zip().map(|_| ())
    }

    fn require_pecmd(&self) -> io::Result<()> {
        self.pecmd().map(|_| ())
    }

    fn verify_shell_context(&self) -> io::Result<()> {
        verify_shell_context_impl()
    }

    fn list_archive(&self, source: &Path) -> io::Result<Vec<ArchiveEntry>> {
        let arguments = vec![
            OsString::from("l"),
            OsString::from("-slt"),
            OsString::from("-sccUTF-8"),
            OsString::from("--"),
            OsString::from(source),
        ];
        let output = run_process(
            &self.seven_zip()?,
            &arguments,
            source,
            Duration::from_secs(60),
        )?;
        parse_listing(&output)
    }

    fn extract_archive_entries(
        &self,
        source: &Path,
        destination: &Path,
        entries: &[String],
    ) -> io::Result<()> {
        let mut arguments = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-aos"),
            OsString::from("-bd"),
            OsString::from("-bb0"),
            OsString::from("-spd"),
        ];
        let mut output = OsString::from("-o");
        output.push(destination);
        arguments.push(output);
        arguments.push(OsString::from("--"));
        arguments.push(OsString::from(source));
        for entry in entries {
            arguments.push(OsString::from(entry));
        }
        run_process(
            &self.seven_zip()?,
            &arguments,
            source,
            Duration::from_secs(300),
        )
        .map(|_| ())
    }

    fn decode_jpeg(&self, bytes: &[u8]) -> io::Result<()> {
        decode_image(bytes, Some(image::ImageFormat::Jpeg))
    }

    fn decode_icon(&self, bytes: &[u8]) -> io::Result<()> {
        decode_image(bytes, Some(image::ImageFormat::Ico))
    }

    fn decode_plain_image(&self, bytes: &[u8]) -> io::Result<()> {
        decode_image(bytes, None)
    }

    fn validate_cursor_file(&self, path: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyCursor, LoadCursorFromFileW};

        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { LoadCursorFromFileW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            DestroyCursor(handle);
        }
        Ok(())
    }

    fn apply_wallpaper(&self, image: &Path) -> io::Result<()> {
        run_process_checked(
            &self.pecmd()?,
            &[OsString::from("WALL"), OsString::from(image)],
            &system_work_directory(),
            Duration::from_secs(60),
        )
    }

    fn execute_esc(&self, script: &Path) -> io::Result<()> {
        run_process_checked(
            &self.pecmd()?,
            &[OsString::from("LOAD"), pecmd_compatible_path(script)],
            &system_work_directory(),
            Duration::from_secs(60),
        )
    }

    fn snapshot_cursors(&self, target_scheme: &str) -> io::Result<CursorSnapshot> {
        let mut slots = std::array::from_fn(|_| None);
        let default_scheme = match open_cursors_key(false) {
            Ok(cursors_key) => {
                let values_result: io::Result<Option<RegistryValueSnapshot>> = (|| {
                    let names = BASE_SLOTS
                        .iter()
                        .chain(OPTIONAL_SLOTS.iter())
                        .map(|(_, registry_name)| *registry_name);
                    for (slot, registry_name) in names.enumerate() {
                        slots[slot] = read_registry_value(cursors_key, registry_name)?;
                    }
                    read_registry_value(cursors_key, "")
                })();
                unsafe {
                    windows_sys::Win32::System::Registry::RegCloseKey(cursors_key);
                }
                values_result?
            }
            // 精简 PE 可能尚未初始化整个 Cursors 键；其快照等价于所有值缺失。
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let scheme_value = match open_schemes_key(false) {
            Ok(schemes_key) => {
                let value = read_registry_value(schemes_key, target_scheme);
                unsafe {
                    windows_sys::Win32::System::Registry::RegCloseKey(schemes_key);
                }
                value?
            }
            // 精简 PE 可能从未创建过 Schemes 子键；这与目标方案值不存在等价，
            // 提交阶段会通过写模式创建该键。
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(CursorSnapshot {
            slots,
            default_scheme,
            target_scheme: scheme_value,
            scheme_name: target_scheme.to_owned(),
        })
    }

    fn write_cursor_slots(&self, values: &[Option<String>; 17]) -> io::Result<()> {
        let key = open_cursors_key(true)?;
        let result = (|| {
            let names = BASE_SLOTS
                .iter()
                .chain(OPTIONAL_SLOTS.iter())
                .map(|(_, registry_name)| *registry_name);
            for (slot, registry_name) in names.enumerate() {
                if let Some(value) = &values[slot] {
                    set_registry_wide(key, registry_name, value, REG_EXPAND_SZ)?;
                }
            }
            Ok(())
        })();
        unsafe {
            windows_sys::Win32::System::Registry::RegCloseKey(key);
        }
        result
    }

    fn write_cursor_scheme(&self, name: &str, values: &[Option<String>; 17]) -> io::Result<()> {
        let key = open_schemes_key(true)?;
        let result = set_registry_wide(key, name, &scheme_field_string(values), REG_SZ);
        unsafe {
            windows_sys::Win32::System::Registry::RegCloseKey(key);
        }
        result
    }

    fn write_cursor_default_scheme(&self, name: &str) -> io::Result<()> {
        let key = open_cursors_key(true)?;
        let result = set_registry_wide(key, "", name, REG_SZ);
        unsafe {
            windows_sys::Win32::System::Registry::RegCloseKey(key);
        }
        result
    }

    fn restore_cursors(&self, snapshot: &CursorSnapshot) -> io::Result<()> {
        let mut failures = Vec::new();
        match open_cursors_key(true) {
            Ok(key) => {
                let names = BASE_SLOTS
                    .iter()
                    .chain(OPTIONAL_SLOTS.iter())
                    .map(|(_, registry_name)| *registry_name);
                for (slot, registry_name) in names.enumerate() {
                    let result = match &snapshot.slots[slot] {
                        Some(value) => set_registry_value(key, registry_name, value),
                        None => delete_registry_wide(key, registry_name),
                    };
                    if let Err(error) = result {
                        failures.push(format!("restore cursor value {registry_name}: {error}"));
                    }
                }
                let default_result = match &snapshot.default_scheme {
                    Some(value) => set_registry_value(key, "", value),
                    None => delete_registry_wide(key, ""),
                };
                if let Err(error) = default_result {
                    failures.push(format!("restore the active cursor scheme: {error}"));
                }
                unsafe {
                    windows_sys::Win32::System::Registry::RegCloseKey(key);
                }
            }
            Err(error) => failures.push(format!("open the cursor registry key: {error}")),
        }

        match open_schemes_key(true) {
            Ok(key) => {
                let result = match &snapshot.target_scheme {
                    Some(value) => set_registry_value(key, &snapshot.scheme_name, value),
                    None => delete_registry_wide(key, &snapshot.scheme_name),
                };
                if let Err(error) = result {
                    failures.push(format!(
                        "restore cursor scheme {}: {error}",
                        snapshot.scheme_name
                    ));
                }
                unsafe {
                    windows_sys::Win32::System::Registry::RegCloseKey(key);
                }
            }
            Err(error) => failures.push(format!("open the cursor schemes registry key: {error}")),
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
    }

    fn publish_cursor_directory(&self, source: &Path, id: &str) -> io::Result<PathBuf> {
        let paths = self.paths()?;
        let destination = paths.cursor_root.join(id);
        super::transaction::ensure_safe_publish_path(&paths.cursor_root, &destination)?;
        publish_directory_atomically(source, &destination)?;
        Ok(destination)
    }

    fn remove_cursor_directory(&self, directory: &Path) -> io::Result<()> {
        match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn refresh_cursors(&self) -> io::Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_SETCURSORS, SPIF_SENDCHANGE, SystemParametersInfoW,
        };

        let succeeded =
            unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, ptr::null_mut(), SPIF_SENDCHANGE) };
        if succeeded == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn modify_shortcut_icons(
        &self,
        changes: &[(PathBuf, PathBuf)],
    ) -> io::Result<Vec<io::Result<bool>>> {
        crate::shell::desktop_icon::reconcile_icon_locations(changes)
    }

    fn notify_shortcuts(&self, links: &[PathBuf]) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SHCNE_UPDATEITEM, SHCNF_FLUSH, SHCNF_PATHW, SHChangeNotify,
        };

        for link in links {
            let wide = link
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            unsafe {
                SHChangeNotify(
                    SHCNE_UPDATEITEM as i32,
                    SHCNF_PATHW | SHCNF_FLUSH,
                    wide.as_ptr() as *const core::ffi::c_void,
                    ptr::null(),
                );
            }
        }
        // SHChangeNotify 没有返回值；SHCNF_FLUSH 保证调用在通知处理完成后返回。
        Ok(())
    }

    fn shell_is_running(&self) -> io::Result<bool> {
        Ok(shell_window_pid()? != 0)
    }

    fn stop_shell(&self) -> io::Result<()> {
        stop_shell_impl()
    }

    fn start_shell(&self) -> io::Result<()> {
        start_shell_impl()
    }

    fn with_temporary_write_permission(
        &self,
        file: &Path,
        operation: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        with_temporary_write_permission_impl(file, operation)
    }
}

/// Windows 主题提交锁（named mutex）。进程异常退出后由内核句柄生命周期自动释放。
pub struct ThemeApplyLock {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl ThemeApplyLock {
    pub fn new() -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = THEME_APPLY_MUTEX_NAME
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }

    /// 有界等待；超时返回 WouldBlock，进程异常退出遗留的 abandoned 状态视为可取得。
    pub fn acquire(&self) -> io::Result<ThemeApplyGuard<'_>> {
        use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let status = unsafe { WaitForSingleObject(self.handle, THEME_APPLY_WAIT_MILLIS) };
        if status == WAIT_OBJECT_0 || status == WAIT_ABANDONED {
            Ok(ThemeApplyGuard { lock: self })
        } else if status == WAIT_TIMEOUT {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another theme apply is in progress; try again later",
            ))
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Default for ThemeApplyLock {
    fn default() -> Self {
        Self {
            handle: ptr::null_mut(),
        }
    }
}

impl Drop for ThemeApplyLock {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

pub struct ThemeApplyGuard<'a> {
    lock: &'a ThemeApplyLock,
}

impl Drop for ThemeApplyGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.lock.handle);
        }
    }
}

// ---------------------------------------------------------------------------
// 路径解析
// ---------------------------------------------------------------------------

fn resolve_theme_paths() -> io::Result<ThemePaths> {
    use std::env;

    let system_root = env::var_os("SystemRoot").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SystemRoot is not set in the Windows PE environment",
        )
    })?;
    let system_root = PathBuf::from(system_root);
    let system_volume = system_root
        .parent()
        .map(Path::to_owned)
        .unwrap_or_else(|| PathBuf::from("\\"));
    let session_root = system_volume.join("Users").join("Theme").join("eli");
    let desktop_roots = resolve_desktop_roots(&system_volume)?;
    let icon_cache_dir = resolve_icon_cache_dir()?;
    Ok(ThemePaths {
        system_root: system_root.clone(),
        staging_root: session_root.join("staging"),
        wallpaper_dir: session_root.join("wallpaper"),
        icon_root: system_volume.join("Users").join("Icon"),
        cursor_root: system_root.join("Cursors").join("Edgeless"),
        desktop_roots,
        icon_cache_dir,
    })
}

fn resolve_desktop_roots(system_volume: &Path) -> io::Result<Vec<PathBuf>> {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop};

    let mut roots = Vec::new();
    if let Ok(current) = known_folder_path(&FOLDERID_Desktop) {
        push_unique_windows_path(&mut roots, current);
    }
    if let Ok(public) = known_folder_path(&FOLDERID_PublicDesktop) {
        push_unique_windows_path(&mut roots, public);
    }
    let compat = system_volume.join("Users").join("Default").join("Desktop");
    if compat.is_dir() {
        push_unique_windows_path(&mut roots, compat);
    }
    Ok(roots)
}

fn push_unique_windows_path(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    let folded = super::transaction::case_fold(&candidate.to_string_lossy());
    if !roots
        .iter()
        .any(|root| super::transaction::case_fold(&root.to_string_lossy()) == folded)
    {
        roots.push(candidate);
    }
}

/// PECMD 2012 不识别 `std::fs::canonicalize` 返回的 `\\?\` 扩展路径，
/// 并且会在未加载脚本时仍返回成功。调用外部 PECMD 前转换为等价的普通绝对
/// 路径；UNC 路径同时恢复为双反斜杠形式。
fn pecmd_compatible_path(path: &Path) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let verbatim = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    let verbatim_unc = [
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    if wide.starts_with(&verbatim_unc) {
        let mut compatible = vec![b'\\' as u16, b'\\' as u16];
        compatible.extend_from_slice(&wide[verbatim_unc.len()..]);
        OsString::from_wide(&compatible)
    } else if wide.starts_with(&verbatim) {
        OsString::from_wide(&wide[verbatim.len()..])
    } else {
        path.as_os_str().to_owned()
    }
}

fn resolve_icon_cache_dir() -> io::Result<PathBuf> {
    use std::env;
    use windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData;

    let local = env::var_os("LocalAppData")
        .map(PathBuf::from)
        .or_else(|| known_folder_path(&FOLDERID_LocalAppData).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LocalAppData is not available in the Windows PE environment",
            )
        })?;
    Ok(local.join("Microsoft").join("Windows").join("Explorer"))
}

fn known_folder_path(id: &windows_sys::core::GUID) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut raw: windows_sys::core::PWSTR = ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut raw) };
    if hr != 0 {
        return Err(io::Error::other(format!(
            "SHGetKnownFolderPath failed with HRESULT 0x{:08x}",
            hr
        )));
    }
    if raw.is_null() {
        return Err(io::Error::other(
            "SHGetKnownFolderPath succeeded without returning a path",
        ));
    }
    let value = unsafe {
        let mut length = 0usize;
        while *raw.add(length) != 0 {
            length += 1;
        }
        let slice = std::slice::from_raw_parts(raw, length);
        let value = OsString::from_wide(slice);
        CoTaskMemFree(raw as *const core::ffi::c_void);
        value
    };
    Ok(PathBuf::from(value))
}

fn system_work_directory() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| Path::new(&root).join("System32"))
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32"))
}

// ---------------------------------------------------------------------------
// 进程执行（7-Zip / PECMD）
// ---------------------------------------------------------------------------

fn run_process(
    executable: &Path,
    arguments: &[OsString],
    context: &Path,
    timeout: std::time::Duration,
) -> io::Result<String> {
    let (status, stdout, stderr) =
        run_process_output(executable, arguments, None, context, timeout)?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(io::Error::other(format!(
            "{} failed with {status}: {}",
            executable.display(),
            truncate_text(&detail, 512)
        )));
    }
    String::from_utf8(stdout).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} returned output that is not valid UTF-8: {error}",
                executable.display()
            ),
        )
    })
}

fn run_process_checked(
    executable: &Path,
    arguments: &[OsString],
    working_directory: &Path,
    timeout: std::time::Duration,
) -> io::Result<()> {
    let context = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let (status, _stdout, stderr) = run_process_output(
        executable,
        arguments,
        Some(working_directory),
        Path::new(&context),
        timeout,
    )?;
    if status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&stderr);
        Err(io::Error::other(format!(
            "{} exited with {status}: {}",
            executable.display(),
            truncate_text(&detail, 512)
        )))
    }
}

/// 在等待子进程时并发排空 stdout/stderr，避免 7-Zip 大清单填满管道后死锁。
fn run_process_output(
    executable: &Path,
    arguments: &[OsString],
    working_directory: Option<&Path>,
    context: &Path,
    timeout: std::time::Duration,
) -> io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    let mut child = command.spawn().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to start {}: {error}", executable.display()),
        )
    })?;
    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} exceeded the time limit while processing {}",
                        executable.display(),
                        context.display()
                    ),
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(io::Error::new(
                    error.kind(),
                    format!("failed to wait for {}: {error}", executable.display()),
                ));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("stdout reader thread panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("stderr reader thread panicked"))??;
    Ok((status, stdout, stderr))
}

fn truncate_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let mut result = text.chars().take(limit).collect::<String>();
        result.push_str("...");
        result
    }
}

// ---------------------------------------------------------------------------
// 图片解码（预检）
// ---------------------------------------------------------------------------

fn decode_image(bytes: &[u8], required_format: Option<image::ImageFormat>) -> io::Result<()> {
    use image::ImageReader;
    use std::io::Cursor;

    let reader = ImageReader::new(Cursor::new(bytes));
    let reader = reader
        .with_guessed_format()
        .map_err(|error| io::Error::other(format!("failed to detect the image format: {error}")))?;
    let Some(actual) = reader.format() else {
        return Err(io::Error::other(
            "the input is not a supported static image format",
        ));
    };
    if let Some(required) = required_format
        && actual != required
    {
        return Err(io::Error::other(format!(
            "expected {required:?} image content but the input is {actual:?}"
        )));
    }
    let image = reader.decode().map_err(|error| {
        io::Error::other(format!("failed to decode the image content: {error}"))
    })?;
    if image.width() == 0 || image.height() == 0 {
        return Err(io::Error::other("the image has a zero dimension"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 注册表
// ---------------------------------------------------------------------------

const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;

type RegistryKey = windows_sys::Win32::System::Registry::HKEY;

fn open_cursors_key(write: bool) -> io::Result<RegistryKey> {
    open_registry_key(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        "Control Panel\\Cursors",
        write,
    )
}

fn open_schemes_key(write: bool) -> io::Result<RegistryKey> {
    open_registry_key(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        "Control Panel\\Cursors\\Schemes",
        write,
    )
}

fn open_registry_key(root: RegistryKey, key_path: &str, write: bool) -> io::Result<RegistryKey> {
    use windows_sys::Win32::System::Registry::{
        KEY_READ, KEY_SET_VALUE, RegCreateKeyExW, RegOpenKeyExW,
    };

    let path = key_path.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut key: RegistryKey = ptr::null_mut();
    let access = if write {
        KEY_READ | KEY_SET_VALUE
    } else {
        KEY_READ
    };
    let status = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, access, &mut key) };
    let status = status as i32;
    if status == 0 {
        return Ok(key);
    }
    if !write {
        return Err(io::Error::from_raw_os_error(status));
    }
    // 写模式：键不存在时创建。
    let status = unsafe {
        RegCreateKeyExW(
            root,
            path.as_ptr(),
            0,
            ptr::null(),
            0,
            access,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status == 0 {
        Ok(key)
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn read_registry_value(
    key: RegistryKey,
    value_name: &str,
) -> io::Result<Option<RegistryValueSnapshot>> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::RegQueryValueExW;

    let name = value_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut length = 0u32;
    let mut value_type = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut value_type,
            ptr::null_mut(),
            &mut length,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let mut buffer = vec![0u8; length as usize];
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut value_type,
            buffer.as_mut_ptr(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    buffer.truncate(length as usize);
    Ok(Some(RegistryValueSnapshot {
        value_type,
        data: buffer,
    }))
}

fn set_registry_wide(
    key: RegistryKey,
    value_name: &str,
    value: &str,
    value_type: u32,
) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::RegSetValueExW;

    let name = value_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut wide = value.encode_utf16().collect::<Vec<_>>();
    wide.push(0);
    let bytes = wide
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect::<Vec<_>>();
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            value_type,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn set_registry_value(
    key: RegistryKey,
    value_name: &str,
    value: &RegistryValueSnapshot,
) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::RegSetValueExW;

    let name = value_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let data = if value.data.is_empty() {
        ptr::null()
    } else {
        value.data.as_ptr()
    };
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            value.value_type,
            data,
            value.data.len() as u32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

fn delete_registry_wide(key: RegistryKey, value_name: &str) -> io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::RegDeleteValueW;

    let name = value_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    if status == 0 || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

// ---------------------------------------------------------------------------
// Shell 生命周期与用户/会话上下文
// ---------------------------------------------------------------------------

fn shell_window_pid() -> io::Result<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};

    let class = "Shell_TrayWnd"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let window = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if window.is_null() {
        return Ok(0);
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    Ok(pid)
}

fn verify_shell_context_impl() -> io::Result<()> {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let pid = shell_window_pid()?;
    if pid == 0 {
        // 没有 Explorer：启动早期，允许写入当前用户 hive。按 SDD 记录用户/会话即可。
        return Ok(());
    }
    let mut shell_session = 0u32;
    let succeeded = unsafe { ProcessIdToSessionId(pid, &mut shell_session) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    let our_session = session_of_current_process()?;
    if shell_session != our_session {
        return Err(io::Error::other(format!(
            "Explorer belongs to session {shell_session} but this process is in session {our_session}; refusing to modify the wrong user hive"
        )));
    }
    let our_sid = process_user_sid(unsafe { GetCurrentProcessId() })?;
    let shell_sid = process_user_sid(pid)?;
    let equal = unsafe {
        windows_sys::Win32::Security::EqualSid(
            our_sid.as_ptr() as *mut core::ffi::c_void,
            shell_sid.as_ptr() as *mut core::ffi::c_void,
        )
    };
    if equal == 0 {
        return Err(io::Error::other(
            "Explorer belongs to a different user; refusing to modify the wrong user hive",
        ));
    }
    Ok(())
}

fn session_of_current_process() -> io::Result<u32> {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let mut session = 0u32;
    let succeeded = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(session)
    }
}

fn process_user_sid(pid: u32) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let current = unsafe { GetCurrentProcessId() };
    let process = if pid == current {
        unsafe { GetCurrentProcess() }
    } else {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        handle
    };
    let mut token: windows_sys::Win32::Foundation::HANDLE = ptr::null_mut();
    let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    if pid != current {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(process);
        }
    }
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut needed = 0u32;
    let queried = unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if queried == 0 && needed == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(token);
        }
        return Err(error);
    }
    let mut buffer = vec![0u8; needed as usize];
    let mut used = 0u32;
    let succeeded = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            buffer.len() as u32,
            &mut used,
        )
    };
    let query_error = if succeeded == 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(token);
    }
    if let Some(error) = query_error {
        return Err(error);
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    if user.User.Sid.is_null() {
        return Err(io::Error::other(
            "the process token contains a null user SID",
        ));
    }
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    let mut sid = vec![0u8; length];
    sid.copy_from_slice(unsafe { std::slice::from_raw_parts(user.User.Sid as *const u8, length) });
    Ok(sid)
}

fn stop_shell_impl() -> io::Result<()> {
    verify_shell_context_impl()?;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    let pids = explorer_process_ids_in_current_session()?;
    let mut failures = Vec::new();
    for pid in pids {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            failures.push(format!(
                "failed to open Explorer process {pid}: {}",
                io::Error::last_os_error()
            ));
            continue;
        }
        let terminated = unsafe { TerminateProcess(handle, 0) };
        if terminated == 0 {
            failures.push(format!(
                "failed to terminate Explorer process {pid}: {}",
                io::Error::last_os_error()
            ));
        } else {
            let wait_status = unsafe { WaitForSingleObject(handle, 15_000) };
            if wait_status != WAIT_OBJECT_0 {
                failures.push(format!(
                    "Explorer process {pid} did not exit within the time limit"
                ));
            }
        }
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

/// 枚举当前会话中的全部 Explorer 进程。只依赖 Shell_TrayWnd 会漏掉 Winlogon
/// 刚拉起、尚未创建任务栏窗口但已经打开图标缓存的 Explorer。
fn explorer_process_ids_in_current_session() -> io::Result<Vec<u32>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::EqualSid;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;

    let session = session_of_current_process()?;
    let current_sid =
        process_user_sid(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() })?;
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut pids = Vec::new();
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while has_entry {
        let name_end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = OsString::from_wide(&entry.szExeFile[..name_end]);
        if name.to_string_lossy().eq_ignore_ascii_case("explorer.exe") {
            let mut process_session = 0u32;
            let found_session =
                unsafe { ProcessIdToSessionId(entry.th32ProcessID, &mut process_session) } != 0;
            if found_session
                && process_session == session
                && let Ok(process_sid) = process_user_sid(entry.th32ProcessID)
            {
                let same_user = unsafe {
                    EqualSid(
                        current_sid.as_ptr() as *mut core::ffi::c_void,
                        process_sid.as_ptr() as *mut core::ffi::c_void,
                    )
                } != 0;
                if same_user {
                    pids.push(entry.th32ProcessID);
                }
            }
        }
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(pids)
}

fn start_shell_impl() -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    // Winlogon 会异步自动拉起 Explorer。先给它一个短暂恢复窗口，避免检查
    // 与自动启动之间的竞态：多启动的 explorer.exe 会被已有 Shell 解释为
    // “打开此电脑”，并在多次主题应用后不断堆积窗口。
    if wait_for_shell_window(std::time::Duration::from_millis(
        SHELL_AUTO_RESTART_GRACE_MILLIS as u64,
    ))? {
        return Ok(());
    }

    let explorer = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("explorer.exe");
    let application = explorer
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut command_line = application.clone();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut information = PROCESS_INFORMATION::default();
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            0,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(information.hThread);
        windows_sys::Win32::Foundation::CloseHandle(information.hProcess);
    }
    if wait_for_shell_window(std::time::Duration::from_millis(SHELL_WAIT_MILLIS as u64))? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Explorer was started but the desktop window did not become ready in time",
        ))
    }
}

/// 在限定时间内等待当前会话的 Shell 窗口出现。
fn wait_for_shell_window(timeout: std::time::Duration) -> io::Result<bool> {
    wait_for_shell_window_with(timeout, std::time::Duration::from_millis(100), || {
        Ok(shell_window_pid()? != 0)
    })
}

fn wait_for_shell_window_with(
    timeout: std::time::Duration,
    interval: std::time::Duration,
    mut probe: impl FnMut() -> io::Result<bool>,
) -> io::Result<bool> {
    let started = std::time::Instant::now();
    loop {
        if probe()? {
            return Ok(true);
        }
        if started.elapsed() >= timeout {
            return Ok(false);
        }
        std::thread::sleep(interval);
    }
}

// ---------------------------------------------------------------------------
// ESS 临时 ACL 提升
// ---------------------------------------------------------------------------

fn with_temporary_write_permission_impl(
    file: &Path,
    operation: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME,
    };
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_GENERIC_WRITE};

    let wide = file
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut owner: PSID = ptr::null_mut();
    let mut group: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut sacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            &mut group,
            &mut dacl,
            &mut sacl,
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::new(
            io::Error::from_raw_os_error(status as i32).kind(),
            format!(
                "GetNamedSecurityInfoW failed for {}: {}",
                file.display(),
                io::Error::from_raw_os_error(status as i32)
            ),
        ));
    }
    let descriptor_guard = LocalAllocation(descriptor);
    let sid =
        process_user_sid(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() })?;
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_WRITE | DELETE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid.as_ptr() as *mut u16,
        },
    };
    let mut new_dacl: *mut ACL = ptr::null_mut();
    let status = unsafe { SetEntriesInAclW(1, &access, dacl, &mut new_dacl) };
    if status != 0 {
        return Err(io::Error::new(
            io::Error::from_raw_os_error(status as i32).kind(),
            format!(
                "SetEntriesInAclW failed for {}: {}",
                file.display(),
                io::Error::from_raw_os_error(status as i32)
            ),
        ));
    }
    let new_dacl_guard = LocalAllocation(new_dacl as *mut core::ffi::c_void);
    let status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_dacl,
            ptr::null_mut(),
        )
    };
    if status == 0 {
        let operation_result = operation();
        let restore_status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        return finish_temporary_permission(operation_result, restore_status);
    }
    if status != windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED {
        return Err(io::Error::new(
            io::Error::from_raw_os_error(status as i32).kind(),
            format!(
                "setting the temporary DACL failed for {}: {}",
                file.display(),
                io::Error::from_raw_os_error(status as i32)
            ),
        ));
    }

    // 受保护文件通常不允许直接改 DACL。先确保两个特权都可用，避免接管所有权后
    // 才发现无法恢复原所有者；随后只在本次操作期间临时接管并在末尾完整还原。
    let _take_ownership = enable_process_privilege(SE_TAKE_OWNERSHIP_NAME).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to enable SeTakeOwnershipPrivilege: {error}"),
        )
    })?;
    let _restore = enable_process_privilege(SE_RESTORE_NAME).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to enable SeRestorePrivilege: {error}"),
        )
    })?;
    let owner_status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            sid.as_ptr() as PSID,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if owner_status != 0 {
        return Err(io::Error::new(
            io::Error::from_raw_os_error(owner_status as i32).kind(),
            format!(
                "taking temporary ownership of {} failed: {}",
                file.display(),
                io::Error::from_raw_os_error(owner_status as i32)
            ),
        ));
    }
    let dacl_status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_dacl,
            ptr::null_mut(),
        )
    };
    if dacl_status != 0 {
        let restore_status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                owner,
                group,
                dacl,
                ptr::null_mut(),
            )
        };
        return finish_temporary_permission(
            Err(io::Error::new(
                io::Error::from_raw_os_error(dacl_status as i32).kind(),
                format!(
                    "setting the temporary DACL after taking ownership of {} failed: {}",
                    file.display(),
                    io::Error::from_raw_os_error(dacl_status as i32)
                ),
            )),
            restore_status,
        );
    }
    let operation_result = operation();
    let restore_status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            owner,
            group,
            dacl,
            ptr::null_mut(),
        )
    };
    drop(new_dacl_guard);
    drop(descriptor_guard);
    finish_temporary_permission(operation_result, restore_status)
}

fn finish_temporary_permission(
    operation_result: io::Result<()>,
    restore_status: u32,
) -> io::Result<()> {
    let restore_error =
        (restore_status != 0).then(|| io::Error::from_raw_os_error(restore_status as i32));
    match (operation_result, restore_error) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(restore_error)) => Err(io::Error::new(
            restore_error.kind(),
            format!(
                "operation succeeded but restoring the original security descriptor failed: {restore_error}"
            ),
        )),
        (Err(operation_error), None) => Err(operation_error),
        (Err(operation_error), Some(restore_error)) => Err(io::Error::new(
            operation_error.kind(),
            format!(
                "{operation_error}; restoring the original security descriptor also failed: {restore_error}"
            ),
        )),
    }
}

struct TokenPrivilegeGuard {
    token: windows_sys::Win32::Foundation::HANDLE,
    previous: windows_sys::Win32::Security::TOKEN_PRIVILEGES,
}

fn enable_process_privilege(name: windows_sys::core::PCWSTR) -> io::Result<TokenPrivilegeGuard> {
    use windows_sys::Win32::Foundation::{
        ERROR_NOT_ALL_ASSIGNED, GetLastError, LUID, SetLastError,
    };
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = ptr::null_mut();
    let opened = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut luid = LUID::default();
    let looked_up = unsafe { LookupPrivilegeValueW(ptr::null(), name, &mut luid) };
    if looked_up == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(token);
        }
        return Err(error);
    }
    let requested = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let mut previous = TOKEN_PRIVILEGES::default();
    let mut previous_length = 0u32;
    unsafe {
        SetLastError(0);
    }
    let adjusted = unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &requested,
            std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
            &mut previous,
            &mut previous_length,
        )
    };
    let last_error = unsafe { GetLastError() };
    if adjusted == 0 || last_error == ERROR_NOT_ALL_ASSIGNED {
        let error = if last_error == 0 {
            io::Error::last_os_error()
        } else {
            io::Error::from_raw_os_error(last_error as i32)
        };
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(token);
        }
        return Err(error);
    }
    Ok(TokenPrivilegeGuard { token, previous })
}

impl Drop for TokenPrivilegeGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Security::AdjustTokenPrivileges(
                self.token,
                0,
                &self.previous,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            windows_sys::Win32::Foundation::CloseHandle(self.token);
        }
    }
}

/// GetNamedSecurityInfoW 与 SetEntriesInAclW 返回的缓冲区由 LocalFree 释放。
struct LocalAllocation(*mut core::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const PROCESS_MUTEX_MARKER: &str = "ELI_THEME_MUTEX_TEST_MARKER";

    #[test]
    fn temporary_permission_reports_restore_failures_without_hiding_the_operation_result() {
        let restore_code = windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

        let error = finish_temporary_permission(Ok(()), restore_code).unwrap_err();
        assert!(error.to_string().contains("operation succeeded"));
        assert!(error.to_string().contains("security descriptor"));

        let error =
            finish_temporary_permission(Err(io::Error::other("replace failed")), restore_code)
                .unwrap_err();
        assert!(error.to_string().contains("replace failed"));
        assert!(error.to_string().contains("also failed"));
    }

    #[test]
    fn pecmd_paths_drop_the_unsupported_verbatim_prefix() {
        assert_eq!(
            pecmd_compatible_path(Path::new(r"\\?\C:\Edgeless\firpe ep\StartIsBackConfig.esc")),
            OsString::from(r"C:\Edgeless\firpe ep\StartIsBackConfig.esc")
        );
        assert_eq!(
            pecmd_compatible_path(Path::new(r"\\?\UNC\server\share\config.esc")),
            OsString::from(r"\\server\share\config.esc")
        );
        assert_eq!(
            pecmd_compatible_path(Path::new(r"X:\Theme\config.esc")),
            OsString::from(r"X:\Theme\config.esc")
        );
    }

    #[test]
    fn shell_wait_uses_an_existing_auto_restarted_shell_without_launching_early() {
        let mut probes = 0;
        let ready = wait_for_shell_window_with(Duration::from_secs(1), Duration::ZERO, || {
            probes += 1;
            Ok(probes == 3)
        })
        .unwrap();

        assert!(ready);
        assert_eq!(probes, 3);
    }

    #[test]
    fn shell_wait_times_out_when_auto_restart_never_appears() {
        let mut probes = 0;
        let ready = wait_for_shell_window_with(Duration::ZERO, Duration::ZERO, || {
            probes += 1;
            Ok(false)
        })
        .unwrap();

        assert!(!ready);
        assert_eq!(probes, 1);
    }

    #[test]
    fn named_mutex_serializes_theme_commits_between_threads() {
        let first = ThemeApplyLock::new().unwrap();
        let first_guard = first.acquire().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let contender = std::thread::spawn(move || {
            let second = ThemeApplyLock::new().unwrap();
            started_tx.send(()).unwrap();
            let _second_guard = second.acquire().unwrap();
            acquired_tx.send(()).unwrap();
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the contender did not start");
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(first_guard);
        acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the contender did not acquire the released mutex");
        contender.join().unwrap();
    }

    #[test]
    #[ignore = "只由跨进程 named mutex 测试作为子进程调用"]
    fn named_mutex_process_child() {
        let Some(marker) = std::env::var_os(PROCESS_MUTEX_MARKER).map(PathBuf::from) else {
            return;
        };
        std::fs::write(marker.join("started"), b"").unwrap();
        let lock = ThemeApplyLock::new().unwrap();
        let _guard = lock.acquire().unwrap();
        std::fs::write(marker.join("acquired"), b"").unwrap();
    }

    #[test]
    fn named_mutex_serializes_theme_commits_between_processes() {
        let marker = std::env::temp_dir().join(format!(
            "eli-theme-mutex-{}-{}",
            std::process::id(),
            super::super::transaction::unique_transaction_id()
        ));
        std::fs::create_dir_all(&marker).unwrap();
        let lock = ThemeApplyLock::new().unwrap();
        let guard = lock.acquire().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "command::theme::apply::details::windows::tests::named_mutex_process_child",
                "--nocapture",
            ])
            .env(PROCESS_MUTEX_MARKER, &marker)
            .spawn()
            .unwrap();

        let started = std::time::Instant::now();
        while !marker.join("started").is_file() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.join("started").is_file(), "the child did not start");
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !marker.join("acquired").exists(),
            "the child acquired a mutex that the parent still owned"
        );

        drop(guard);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "the mutex child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(marker.join("acquired").is_file());
        std::fs::remove_dir_all(marker).unwrap();
    }
}
