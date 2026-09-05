use super::{PluginAttribute, package};
use crate::Ctx;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 修改选中启动盘中插件包的属性。
pub fn set_attribute(ctx: &Ctx, name: &OsStr, attribute: PluginAttribute) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    set_attribute_in_resource_dir(
        &package::resource_dir(&bootdisk.mount_point),
        name,
        attribute,
    )
}

fn set_attribute_in_resource_dir(
    resource_dir: &Path,
    name: &OsStr,
    attribute: PluginAttribute,
) -> io::Result<PathBuf> {
    let source = package::find(resource_dir, name)?;
    if PluginAttribute::from_path(&source) == Some(attribute) {
        return Ok(source);
    }
    let destination = source.with_extension(attribute.extension());

    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "plugin package already exists with the requested attribute: {}",
                    destination.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    fs::rename(&source, &destination).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to change plugin attribute from {} to {}: {error}",
                source.display(),
                destination.display()
            ),
        )
    })?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::test_resource_dir;

    #[test]
    fn changes_a_plugin_attribute_by_file_stem() {
        let resource = test_resource_dir();
        let source = resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7z");
        let destination = resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7zf");
        fs::write(&source, "package").unwrap();

        let changed = set_attribute_in_resource_dir(
            &resource,
            OsStr::new("搜狗拼音_16.4.0.0_Cno（bot）"),
            PluginAttribute::Frozen,
        )
        .unwrap();

        assert_eq!(changed, destination);
        assert!(!source.exists());
        assert!(destination.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_an_existing_attribute_variant() {
        let resource = test_resource_dir();
        fs::write(resource.join("plugin_1.0_author.7z"), "normal").unwrap();
        fs::write(resource.join("plugin_1.0_author.7zf"), "frozen").unwrap();

        let error = set_attribute_in_resource_dir(
            &resource,
            OsStr::new("plugin_1.0_author.7z"),
            PluginAttribute::Frozen,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn keeps_a_plugin_that_already_has_the_requested_attribute() {
        let resource = test_resource_dir();
        let target = resource.join("plugin_1.0_author.7ZF");
        fs::write(&target, "frozen").unwrap();

        let unchanged = set_attribute_in_resource_dir(
            &resource,
            OsStr::new("plugin_1.0_author.7ZF"),
            PluginAttribute::Frozen,
        )
        .unwrap();

        assert_eq!(unchanged, target);
        assert!(target.exists());
        fs::remove_dir_all(resource).unwrap();
    }
}
