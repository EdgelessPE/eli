use super::package;
use crate::Ctx;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 将插件包复制到选中的启动盘。
pub fn store(ctx: &Ctx, source: &Path) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    store_in_resource_dir(&package::resource_dir(&bootdisk.mount_point), source)
}

fn store_in_resource_dir(resource_dir: &Path, source: &Path) -> io::Result<PathBuf> {
    if !source.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin package is not a file: {}", source.display()),
        ));
    }
    package::parse(source)?;
    let file_name = source.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin package path has no file name: {}", source.display()),
        )
    })?;
    let destination = resource_dir.join(file_name);
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("plugin package already exists: {}", destination.display()),
        ));
    }

    fs::copy(source, &destination).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to store plugin package from {} to {}: {error}",
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
    fn stores_a_plugin_package_without_removing_the_source() {
        let root = test_resource_dir();
        let source_dir = root.join("source");
        let resource = root.join("resource");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&resource).unwrap();
        let source = source_dir.join("工具箱_1.0.0_Edgeless.7zl");
        let destination = resource.join("工具箱_1.0.0_Edgeless.7zl");
        fs::write(&source, "package").unwrap();

        assert_eq!(
            store_in_resource_dir(&resource, &source).unwrap(),
            destination
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "package");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "package");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_an_existing_package() {
        let root = test_resource_dir();
        let source_dir = root.join("source");
        let resource = root.join("resource");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&resource).unwrap();
        let source = source_dir.join("plugin_1.0_author.7z");
        let destination = resource.join("plugin_1.0_author.7z");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();

        let error = store_in_resource_dir(&resource, &source).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(destination).unwrap(), "old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_non_plugin_file() {
        let root = test_resource_dir();
        let resource = root.join("resource");
        fs::create_dir_all(&resource).unwrap();
        let source = root.join("plugin_1.0_author.zip");
        fs::write(&source, "package").unwrap();

        let error = store_in_resource_dir(&resource, &source).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!resource.join("plugin_1.0_author.zip").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
