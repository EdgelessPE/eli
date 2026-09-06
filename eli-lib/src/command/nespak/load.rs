use crate::Ctx;
#[cfg(windows)]
use crate::dependency::ProgramDependency;
use crate::dependency::RuntimeEnvironment;
#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// NesPak 解压后的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    /// 已找到并交由 PECMD 执行 `Nes.ini`。
    Loaded,
    /// 解压完成，但包内未提供 `Nes.ini`。
    Extracted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoadPaths {
    archive: PathBuf,
    destination: PathBuf,
    configuration: PathBuf,
}

impl LoadPaths {
    fn new(archive: &Path, program_files: &Path) -> Self {
        let destination = program_files.join("Edgeless");
        Self {
            archive: archive.to_owned(),
            configuration: destination.join("Nes.ini"),
            destination,
        }
    }
}

trait NesPakLoader {
    fn extract(&self, archive: &Path, destination: &Path) -> io::Result<()>;
    fn load_configuration(&self, configuration: &Path) -> io::Result<()>;
}

/// 导入并加载指定的 NesPak 必要组件包。
///
/// 此命令只有一个共享的 PE 运行目录。Windows 实现使用命名互斥锁串行化同一
/// 机器上来自 eli 的执行；7-Zip 的 `-aos` 同时保证不会覆盖已存在的文件。
pub fn load(ctx: &Ctx, archive: &Path) -> io::Result<LoadStatus> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    load_on_supported_platform(ctx, archive)
}

#[cfg(windows)]
fn load_on_supported_platform(ctx: &Ctx, archive: &Path) -> io::Result<LoadStatus> {
    use std::env;

    let programs = ctx
        .dependencies()
        .require_programs(&[ProgramDependency::SevenZip, ProgramDependency::Pecmd])?;
    let program_files = env::var_os("ProgramFiles").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "ProgramFiles is not set in the Windows PE environment",
        )
    })?;
    let paths = LoadPaths::new(archive, Path::new(&program_files));
    let lock = ExecutionLock::new()?;
    let _guard = lock.acquire()?;
    let loader = SystemNesPakLoader {
        seven_zip: programs.executable(ProgramDependency::SevenZip)?.to_owned(),
        pecmd: programs.executable(ProgramDependency::Pecmd)?.to_owned(),
    };
    load_with(&paths, &loader)
}

#[cfg(not(windows))]
fn load_on_supported_platform(_ctx: &Ctx, _archive: &Path) -> io::Result<LoadStatus> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "NesPak loading is only implemented for Windows PE",
    ))
}

fn load_with(paths: &LoadPaths, loader: &dyn NesPakLoader) -> io::Result<LoadStatus> {
    loader.extract(&paths.archive, &paths.destination)?;
    if paths.configuration.is_file() {
        loader.load_configuration(&paths.configuration)?;
        Ok(LoadStatus::Loaded)
    } else {
        Ok(LoadStatus::Extracted)
    }
}

#[cfg(windows)]
struct SystemNesPakLoader {
    seven_zip: PathBuf,
    pecmd: PathBuf,
}

#[cfg(windows)]
impl NesPakLoader for SystemNesPakLoader {
    fn extract(&self, archive: &Path, destination: &Path) -> io::Result<()> {
        use std::ffi::OsString;
        use std::process::Command;

        let mut output_argument = OsString::from("-o");
        output_argument.push(destination);
        run_checked(
            Command::new(&self.seven_zip)
                .arg("x")
                .arg(archive)
                .arg("-y")
                .arg("-aos")
                .arg(output_argument),
            &format!("extract NesPak archive {}", archive.display()),
        )
    }

    fn load_configuration(&self, configuration: &Path) -> io::Result<()> {
        use std::process::Command;

        // 与 `pecmd.exe Nes.ini` 一致，直接将配置文件作为 PECMD 的首个参数。
        run_checked(
            Command::new(&self.pecmd).arg(configuration),
            &format!("load NesPak configuration {}", configuration.display()),
        )
    }
}

#[cfg(windows)]
fn run_checked(command: &mut std::process::Command, operation: &str) -> io::Result<()> {
    let status = command
        .status()
        .map_err(|error| io::Error::new(error.kind(), format!("failed to {operation}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to {operation}: process exited with {status}"
        )))
    }
}

#[cfg(windows)]
struct ExecutionLock {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ExecutionLock {
    fn new() -> io::Result<Self> {
        use std::ptr;
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = "Local\\Edgeless.eli.nespak-load"
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

    fn acquire(&self) -> io::Result<ExecutionGuard<'_>> {
        use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

        let status = unsafe { WaitForSingleObject(self.handle, INFINITE) };
        if status == WAIT_OBJECT_0 || status == WAIT_ABANDONED {
            Ok(ExecutionGuard { lock: self })
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(windows)]
impl Drop for ExecutionLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
struct ExecutionGuard<'a> {
    lock: &'a ExecutionLock,
}

#[cfg(windows)]
impl Drop for ExecutionGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.lock.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct TestLoader {
        create_configuration: bool,
        calls: Mutex<Vec<String>>,
    }

    impl NesPakLoader for TestLoader {
        fn extract(&self, archive: &Path, destination: &Path) -> io::Result<()> {
            self.calls.lock().unwrap().push(format!(
                "extract:{}:{}",
                archive.display(),
                destination.display()
            ));
            if self.create_configuration {
                fs::create_dir_all(destination)?;
                fs::write(destination.join("Nes.ini"), "test")?;
            }
            Ok(())
        }

        fn load_configuration(&self, configuration: &Path) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("load:{}", configuration.display()));
            Ok(())
        }
    }

    #[test]
    fn derives_paths_from_the_explicit_archive_and_program_files() {
        let paths = LoadPaths::new(
            Path::new("U:\\NesPak\\_Inport.7z"),
            Path::new("X:\\Program Files"),
        );

        assert_eq!(paths.archive, PathBuf::from("U:\\NesPak\\_Inport.7z"));
        assert_eq!(
            paths.destination,
            PathBuf::from("X:\\Program Files\\Edgeless")
        );
        assert_eq!(
            paths.configuration,
            PathBuf::from("X:\\Program Files\\Edgeless\\Nes.ini")
        );
    }

    #[test]
    fn extracts_before_loading_the_configuration() {
        let temporary = temporary_directory();
        let paths = LoadPaths {
            archive: temporary.join("_Inport.7z"),
            destination: temporary.join("Edgeless"),
            configuration: temporary.join("Edgeless").join("Nes.ini"),
        };
        let loader = TestLoader {
            create_configuration: true,
            ..Default::default()
        };

        let status = load_with(&paths, &loader).unwrap();

        assert_eq!(status, LoadStatus::Loaded);
        assert_eq!(loader.calls.lock().unwrap().len(), 2);
        assert!(loader.calls.lock().unwrap()[0].starts_with("extract:"));
        assert!(loader.calls.lock().unwrap()[1].starts_with("load:"));
        fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn skips_pecmd_when_the_archive_does_not_provide_a_configuration() {
        let temporary = temporary_directory();
        let paths = LoadPaths {
            archive: temporary.join("_Inport.7z"),
            destination: temporary.join("Edgeless"),
            configuration: temporary.join("Edgeless").join("Nes.ini"),
        };
        let loader = TestLoader::default();

        let status = load_with(&paths, &loader).unwrap();

        assert_eq!(status, LoadStatus::Extracted);
        assert_eq!(loader.calls.lock().unwrap().len(), 1);
        fs::remove_dir_all(temporary).unwrap();
    }

    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eli-nespak-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
