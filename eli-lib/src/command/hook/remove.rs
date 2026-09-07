use super::{HookStage, hooks_directory, reject_unsafe_hook_path, validate_script_name};
use crate::Ctx;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 从所选启动盘的指定生命周期钩子中删除一个脚本。
pub fn remove(ctx: &Ctx, hook: HookStage, script: &OsStr) -> io::Result<PathBuf> {
    validate_script_name(script)?;
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    remove_from_directory(
        &hooks_directory(&bootdisk.mount_point),
        hook.as_os_str(),
        script,
    )
}

fn remove_from_directory(hooks: &Path, hook: &OsStr, script: &OsStr) -> io::Result<PathBuf> {
    let directory = hooks.join(hook);
    let path = directory.join(script);
    reject_unsafe_hook_path(hooks, hook, Some(&path))?;
    fs::remove_file(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to remove hook script {}: {error}", path.display()),
        )
    })?;
    // 脚本已经成功删除后，空目录清理由其他文件系统参与者决定，不影响删除结果。
    let _ = fs::remove_dir(&directory);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-hook-remove-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn removes_only_the_selected_script_and_cleans_an_empty_hook() {
        let root = test_root();
        let hook = root.join("onExit");
        fs::create_dir_all(&hook).unwrap();
        fs::write(hook.join("one.cmd"), "").unwrap();
        fs::write(hook.join("two.wcs"), "").unwrap();

        remove_from_directory(&root, OsStr::new("onExit"), OsStr::new("one.cmd")).unwrap();
        assert!(!hook.join("one.cmd").exists());
        assert!(hook.join("two.wcs").exists());
        remove_from_directory(&root, OsStr::new("onExit"), OsStr::new("two.wcs")).unwrap();
        assert!(!hook.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_remove_through_a_symbolic_hook_directory() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let hooks = root.join("Edgeless").join("Hooks");
        let outside = root.join("outside");
        fs::create_dir_all(&hooks).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("save.cmd"), "keep").unwrap();
        symlink(&outside, hooks.join("onExit")).unwrap();

        let error = remove_from_directory(&hooks, OsStr::new("onExit"), OsStr::new("save.cmd"))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(outside.join("save.cmd")).unwrap(),
            "keep"
        );
        fs::remove_file(hooks.join("onExit")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn refuses_to_remove_through_a_junction_hook_directory() {
        use std::process::{Command, Stdio};

        let root = test_root();
        let hooks = root.join("Edgeless").join("Hooks");
        let outside = root.join("outside");
        let junction = hooks.join("onExit");
        fs::create_dir_all(&hooks).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("save.cmd"), "keep").unwrap();
        let status = Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&outside)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let error = remove_from_directory(&hooks, OsStr::new("onExit"), OsStr::new("save.cmd"))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(outside.join("save.cmd")).unwrap(),
            "keep"
        );
        fs::remove_dir(&junction).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
