use super::{HookStage, hooks_directory, reject_unsafe_hook_path, validate_script_name};
use crate::Ctx;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 将脚本添加到所选启动盘的指定生命周期钩子中。
pub fn add(ctx: &Ctx, hook: HookStage, source: &Path) -> io::Result<PathBuf> {
    if !source.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("hook script must be an existing file: {}", source.display()),
        ));
    }
    let file_name = source.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "hook script path has no file name",
        )
    })?;
    validate_script_name(file_name)?;
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    add_to_directory(
        &hooks_directory(&bootdisk.mount_point),
        hook.as_os_str(),
        source,
        file_name,
    )
}

fn add_to_directory(
    hooks: &Path,
    hook: &OsStr,
    source: &Path,
    file_name: &OsStr,
) -> io::Result<PathBuf> {
    let directory = hooks.join(hook);
    let destination = directory.join(file_name);
    reject_unsafe_hook_path(hooks, hook, Some(&destination))?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("hook script already exists: {}", destination.display()),
        ));
    }
    fs::create_dir_all(&directory)?;
    reject_unsafe_hook_path(hooks, hook, Some(&destination))?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        ".{}-eli-{}-{sequence}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        io::copy(&mut BufReader::new(File::open(source)?), &mut output)?;
        output.flush()?;
        output.sync_all()?;
        fs::rename(&temporary, &destination)?;
        Ok(destination)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-hook-add-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn copies_a_script_without_overwriting_an_existing_entry() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let source = root.join("setup.cmd");
        let hooks = root.join("Hooks");
        fs::write(&source, "first").unwrap();

        let added = add_to_directory(
            &hooks,
            OsStr::new("onExit"),
            &source,
            OsStr::new("setup.cmd"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&added).unwrap(), "first");
        fs::write(&source, "second").unwrap();
        assert_eq!(
            add_to_directory(
                &hooks,
                OsStr::new("onExit"),
                &source,
                OsStr::new("setup.cmd")
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(added).unwrap(), "first");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_add_through_a_symbolic_hook_directory() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let hooks = root.join("Edgeless").join("Hooks");
        let outside = root.join("outside");
        fs::create_dir_all(&hooks).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, hooks.join("onExit")).unwrap();
        let source = root.join("setup.cmd");
        fs::write(&source, "payload").unwrap();

        let error = add_to_directory(
            &hooks,
            OsStr::new("onExit"),
            &source,
            OsStr::new("setup.cmd"),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!outside.join("setup.cmd").exists());
        fs::remove_file(hooks.join("onExit")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn refuses_to_add_through_a_junction_hook_directory() {
        use std::process::{Command, Stdio};

        let root = test_root();
        let hooks = root.join("Edgeless").join("Hooks");
        let outside = root.join("outside");
        let junction = hooks.join("onExit");
        fs::create_dir_all(&hooks).unwrap();
        fs::create_dir_all(&outside).unwrap();
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
        let source = root.join("setup.cmd");
        fs::write(&source, "payload").unwrap();

        let error = add_to_directory(
            &hooks,
            OsStr::new("onExit"),
            &source,
            OsStr::new("setup.cmd"),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!outside.join("setup.cmd").exists());
        fs::remove_dir(&junction).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
