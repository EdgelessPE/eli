mod add;
mod call;
mod list;
mod remove;

pub use add::add;
pub use call::{
    CallOptions, CallPolicy, DEFAULT_DICTIONARY, HookCallResult, HookCallSummary, call,
};
pub use list::{HookScript, list};
pub use remove::remove;

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path};

const HOOKS_DIRECTORY: &str = "Hooks";

/// Edgeless 文档公开的生命周期钩子阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookStage {
    OnDiskFound,
    BeforeLocalBoost,
    BeforePluginLoading,
    OnDesktopShown,
    OnBootFinished,
    OnExit,
}

impl HookStage {
    pub const ALL: [Self; 6] = [
        Self::OnDiskFound,
        Self::BeforeLocalBoost,
        Self::BeforePluginLoading,
        Self::OnDesktopShown,
        Self::OnBootFinished,
        Self::OnExit,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnDiskFound => "onDiskFound",
            Self::BeforeLocalBoost => "beforeLocalBoost",
            Self::BeforePluginLoading => "beforePluginLoading",
            Self::OnDesktopShown => "onDesktopShown",
            Self::OnBootFinished => "onBootFinished",
            Self::OnExit => "onExit",
        }
    }

    fn as_os_str(self) -> &'static OsStr {
        OsStr::new(self.as_str())
    }
}

impl std::fmt::Display for HookStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&OsStr> for HookStage {
    type Error = io::Error;

    fn try_from(value: &OsStr) -> Result<Self, Self::Error> {
        Self::ALL
            .into_iter()
            .find(|stage| value == OsStr::new(stage.as_str()))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "unknown hook stage {:?}; expected one of: {}",
                        value,
                        Self::ALL.map(HookStage::as_str).join(", ")
                    ),
                )
            })
    }
}

fn hooks_directory(mount_point: &Path) -> std::path::PathBuf {
    mount_point.join("Edgeless").join(HOOKS_DIRECTORY)
}

fn validate_script_name(name: &OsStr) -> io::Result<()> {
    validate_portable_file_name(name, "hook script name")?;
    if is_supported_script(Path::new(name)) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "hook scripts must use the .cmd or .wcs extension",
        ))
    }
}

/// 使用所有支持平台都能安全表示的单个文件名，避免路径穿越和 Windows 重解析差异。
fn validate_portable_file_name(name: &OsStr, description: &str) -> io::Result<()> {
    let path = Path::new(name);
    let mut components = path.components();
    let valid_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let text = name.to_str().unwrap_or_default();
    let valid_text = !text.is_empty()
        && !text.ends_with([' ', '.'])
        && !is_windows_reserved_name(text)
        && !text
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character));
    if valid_component && valid_text {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be a portable file name without path separators"),
        ))
    }
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

/// 拒绝钩子目标路径中已存在的符号链接或 Windows 重解析点。
fn reject_unsafe_hook_path(hooks: &Path, hook: &OsStr, target: Option<&Path>) -> io::Result<()> {
    let mut paths = Vec::with_capacity(4);
    if let Some(edgeless) = hooks.parent() {
        paths.push(edgeless);
    }
    paths.push(hooks);
    let hook_directory = hooks.join(hook);
    paths.push(&hook_directory);
    if let Some(target) = target {
        paths.push(target);
    }
    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) if is_reparse_point(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("hook path is a reparse point: {}", path.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn is_supported_script(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("wcs")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ctx;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-hook-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn accepts_documented_hook_stages_and_portable_script_names() {
        for stage in HookStage::ALL {
            assert_eq!(HookStage::try_from(stage.as_os_str()).unwrap(), stage);
        }
        validate_script_name(OsStr::new("初始化.cmd")).unwrap();
        validate_script_name(OsStr::new("preset.WCS")).unwrap();
    }

    #[test]
    fn rejects_unknown_hook_stages_and_unsupported_scripts() {
        for name in [
            "customStage",
            "ONEXIT",
            "../onExit",
            r"parent\onExit",
            "parent/onExit",
            "onExit:",
        ] {
            assert_eq!(
                HookStage::try_from(OsStr::new(name)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(
            validate_script_name(OsStr::new("readme.txt"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn rejects_windows_reserved_device_names() {
        for name in ["CON", "nul.txt", "COM1.cmd", "lpt9.wcs"] {
            assert_eq!(
                validate_portable_file_name(OsStr::new(name), "test name")
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        validate_portable_file_name(OsStr::new("COM10.cmd"), "test name").unwrap();
    }

    #[test]
    fn concurrent_adds_to_the_same_hook_never_overwrite() {
        let root = test_root("concurrent-add");
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        fs::create_dir_all(root.join("Edgeless")).unwrap();
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        fs::write(root.join("Edgeless").join("version.txt"), "test").unwrap();
        let first = first_dir.join("save.cmd");
        let second = second_dir.join("save.cmd");
        fs::write(&first, "first complete payload").unwrap();
        fs::write(&second, "second complete payload").unwrap();
        let ctx = Arc::new(Ctx::new(Some(root.clone())));
        let barrier = Arc::new(Barrier::new(3));
        let workers = [first, second].map(|source| {
            let ctx = Arc::clone(&ctx);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                add(ctx.as_ref(), HookStage::OnExit, &source)
            })
        });

        barrier.wait();
        let results = workers.map(|worker| worker.join().unwrap());

        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "unexpected concurrent add results: {results:?}"
        );
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .filter(|error| error.kind() == io::ErrorKind::AlreadyExists)
                .count(),
            1
        );
        let stored = fs::read_to_string(
            root.join("Edgeless")
                .join("Hooks")
                .join("onExit")
                .join("save.cmd"),
        )
        .unwrap();
        assert!(matches!(
            stored.as_str(),
            "first complete payload" | "second complete payload"
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
