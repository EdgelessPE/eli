use super::package;
use crate::Ctx;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 从选中的启动盘删除插件包。
///
/// `name` 可以是完整文件名，也可以是文件主名。
pub fn delete(ctx: &Ctx, name: &OsStr) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    delete_from_resource_dir(&package::resource_dir(&bootdisk.mount_point), name)
}

fn delete_from_resource_dir(resource_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    let target = package::find(resource_dir, name)?;
    fs::remove_file(&target).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to delete {}: {error}", target.display()),
        )
    })?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::test_resource_dir;
    use std::ffi::OsString;

    #[test]
    fn deletes_a_plugin_by_complete_file_name() {
        let resource = test_resource_dir();
        let name = OsString::from("搜狗拼音_16.4.0.0_Cno（bot）.7zf");
        let target = resource.join(&name);
        fs::write(&target, "package").unwrap();

        assert_eq!(delete_from_resource_dir(&resource, &name).unwrap(), target);
        assert!(!target.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn deletes_a_plugin_by_file_stem() {
        let resource = test_resource_dir();
        let target = resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7zl");
        fs::write(&target, "package").unwrap();

        let deleted =
            delete_from_resource_dir(&resource, OsStr::new("搜狗拼音_16.4.0.0_Cno（bot）"))
                .unwrap();

        assert_eq!(deleted, target);
        assert!(!target.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn rejects_paths_and_preserves_the_file() {
        let resource = test_resource_dir();
        let target = resource.join("plugin_1.0_author.7z");
        fs::write(&target, "package").unwrap();

        let error = delete_from_resource_dir(&resource, OsStr::new("../plugin.7z")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(target.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn ignores_non_package_files() {
        let resource = test_resource_dir();
        fs::write(resource.join("plugin_1.0_author.txt"), "not a package").unwrap();

        let error =
            delete_from_resource_dir(&resource, OsStr::new("plugin_1.0_author")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(resource).unwrap();
    }
}
