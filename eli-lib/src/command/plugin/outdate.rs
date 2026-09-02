use super::{PluginAttribute, package};
use crate::Ctx;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const OUTDATED_DIRECTORY: &str = "过期插件包";

/// 将选中启动盘中的插件包设为冻结属性并移入过期插件包目录。
pub fn outdate(ctx: &Ctx, name: &OsStr) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    outdate_in_resource_dir(&package::resource_dir(&bootdisk.mount_point), name)
}

fn outdate_in_resource_dir(resource_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    let source = package::find(resource_dir, name)?;
    let frozen = if PluginAttribute::from_path(&source) == Some(PluginAttribute::Frozen) {
        source.clone()
    } else {
        source.with_extension(PluginAttribute::Frozen.extension())
    };
    let outdated_dir = resource_dir.join(OUTDATED_DIRECTORY);
    let destination = outdated_dir.join(frozen.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin package has no file name",
        )
    })?);

    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "outdated plugin package already exists: {}",
                destination.display()
            ),
        ));
    }
    if frozen != source && frozen.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "plugin package already exists with the frozen attribute: {}",
                frozen.display()
            ),
        ));
    }
    fs::create_dir_all(&outdated_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to create {}: {error}", outdated_dir.display()),
        )
    })?;

    if frozen != source {
        fs::rename(&source, &frozen).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to change plugin attribute from {} to {}: {error}",
                    source.display(),
                    frozen.display()
                ),
            )
        })?;
    }

    if let Err(error) = fs::rename(&frozen, &destination) {
        if frozen != source {
            let _ = fs::rename(&frozen, &source);
        }
        return Err(io::Error::new(
            error.kind(),
            format!(
                "failed to move outdated plugin package from {} to {}: {error}",
                frozen.display(),
                destination.display()
            ),
        ));
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::test_resource_dir;

    #[test]
    fn freezes_and_moves_a_plugin_by_file_stem() {
        let resource = test_resource_dir();
        let source = resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7z");
        let destination = resource
            .join(OUTDATED_DIRECTORY)
            .join("搜狗拼音_16.4.0.0_Cno（bot）.7zf");
        fs::write(&source, "package").unwrap();

        let moved =
            outdate_in_resource_dir(&resource, OsStr::new("搜狗拼音_16.4.0.0_Cno（bot）")).unwrap();

        assert_eq!(moved, destination);
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(destination).unwrap(), "package");
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn moves_an_already_frozen_plugin() {
        let resource = test_resource_dir();
        let source = resource.join("plugin_1.0_author.7ZF");
        let destination = resource
            .join(OUTDATED_DIRECTORY)
            .join("plugin_1.0_author.7ZF");
        fs::write(&source, "package").unwrap();

        assert_eq!(
            outdate_in_resource_dir(&resource, OsStr::new("plugin_1.0_author.7ZF")).unwrap(),
            destination
        );
        assert!(!source.exists());
        assert!(destination.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_an_outdated_package() {
        let resource = test_resource_dir();
        let source = resource.join("plugin_1.0_author.7z");
        let outdated_dir = resource.join(OUTDATED_DIRECTORY);
        let destination = outdated_dir.join("plugin_1.0_author.7zf");
        fs::create_dir_all(&outdated_dir).unwrap();
        fs::write(&source, "current").unwrap();
        fs::write(&destination, "outdated").unwrap();

        let error =
            outdate_in_resource_dir(&resource, OsStr::new("plugin_1.0_author")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(source).unwrap(), "current");
        assert_eq!(fs::read_to_string(destination).unwrap(), "outdated");
        fs::remove_dir_all(resource).unwrap();
    }
}
