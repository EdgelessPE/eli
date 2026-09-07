use super::{HookStage, hooks_directory, is_supported_script};
use crate::Ctx;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 启动盘中配置的一条生命周期钩子脚本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookScript {
    pub hook: HookStage,
    pub script: OsString,
    pub path: PathBuf,
}

/// 列出所选启动盘中全部有效的 `.cmd` 和 `.wcs` 钩子脚本。
pub fn list(ctx: &Ctx) -> io::Result<Vec<HookScript>> {
    let bootdisk = ctx.bootdisk()?;
    list_in_directory(&hooks_directory(&bootdisk.selected.mount_point))
}

pub(super) fn list_in_directory(hooks: &Path) -> io::Result<Vec<HookScript>> {
    let directories = match fs::read_dir(hooks) {
        Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut scripts = Vec::new();
    for directory in directories {
        if !directory.file_type()?.is_dir() {
            continue;
        }
        let hook_name = directory.file_name();
        let Ok(hook) = HookStage::try_from(hook_name.as_os_str()) else {
            continue;
        };
        for entry in fs::read_dir(directory.path())? {
            let entry = entry?;
            if entry.file_type()?.is_file() && is_supported_script(&entry.path()) {
                scripts.push(HookScript {
                    hook,
                    script: entry.file_name(),
                    path: entry.path(),
                });
            }
        }
    }
    scripts.sort_by(|left, right| {
        left.hook
            .as_str()
            .to_ascii_lowercase()
            .cmp(&right.hook.as_str().to_ascii_lowercase())
            .then_with(|| {
                left.script
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .cmp(&right.script.to_string_lossy().to_ascii_lowercase())
            })
    });
    Ok(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-hook-list-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn lists_supported_scripts_in_stable_hook_and_file_order() {
        let root = test_root();
        fs::create_dir_all(root.join("onExit")).unwrap();
        fs::create_dir_all(root.join("beforePluginLoading")).unwrap();
        fs::create_dir_all(root.join("customStage")).unwrap();
        fs::write(root.join("onExit").join("b.wcs"), "").unwrap();
        fs::write(root.join("onExit").join("a.CMD"), "").unwrap();
        fs::write(root.join("onExit").join("notes.txt"), "").unwrap();
        fs::write(root.join("beforePluginLoading").join("first.cmd"), "").unwrap();
        fs::write(root.join("customStage").join("ignored.cmd"), "").unwrap();

        let listed = list_in_directory(&root).unwrap();

        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].hook, HookStage::BeforePluginLoading);
        assert_eq!(listed[1].script, "a.CMD");
        assert_eq!(listed[2].script, "b.wcs");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_hooks_directory_is_an_empty_configuration() {
        assert!(list_in_directory(&test_root()).unwrap().is_empty());
    }
}
