use crate::Ctx;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// 从选中的启动盘删除插件包。
///
/// `name` 可以是完整的 `.7z` 文件名，也可以是文件 stem。
pub fn delete(ctx: &Ctx, name: &OsStr) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let resource_dir = bootdisk.mount_point.join("Edgeless").join("Resource");
    delete_from_resource_dir(&resource_dir, name)
}

fn delete_from_resource_dir(resource_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    validate_plugin_name(name)?;

    let mut exact_match = None;
    let mut stem_matches = Vec::new();
    for entry in fs::read_dir(resource_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read {}: {error}", resource_dir.display()),
        )
    })? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || !is_plugin_package(&entry.path()) {
            continue;
        }

        if entry.file_name() == name {
            exact_match = Some(entry.path());
            break;
        }
        if entry.path().file_stem() == Some(name) {
            stem_matches.push(entry.path());
        }
    }

    let target = match exact_match {
        Some(path) => path,
        None if stem_matches.len() == 1 => stem_matches.remove(0),
        None if stem_matches.len() > 1 => {
            let matches = stem_matches
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("plugin name is ambiguous: {matches}"),
            ));
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "plugin package was not found in {}: {}",
                    resource_dir.display(),
                    name.to_string_lossy()
                ),
            ));
        }
    };

    fs::remove_file(&target).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to delete {}: {error}", target.display()),
        )
    })?;
    Ok(target)
}

fn validate_plugin_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    let mut components = path.components();
    let is_single_file_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if name.is_empty() || !is_single_file_name {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin must be a file name or file stem, not a path",
        ));
    }
    Ok(())
}

fn is_plugin_package(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("7z"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn resource_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eli-lib-plugin-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn deletes_a_plugin_by_complete_file_name() {
        let resource = resource_dir();
        let name = OsString::from("搜狗拼音_16.4.0.0_Cno（bot）.7z");
        let target = resource.join(&name);
        fs::write(&target, "package").unwrap();

        assert_eq!(delete_from_resource_dir(&resource, &name).unwrap(), target);
        assert!(!target.exists());

        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn deletes_a_plugin_by_file_stem() {
        let resource = resource_dir();
        let target = resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7z");
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
        let resource = resource_dir();
        let target = resource.join("plugin.7z");
        fs::write(&target, "package").unwrap();

        let error = delete_from_resource_dir(&resource, OsStr::new("../plugin.7z")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(target.exists());
        fs::remove_dir_all(resource).unwrap();
    }

    #[test]
    fn ignores_non_package_files() {
        let resource = resource_dir();
        fs::write(resource.join("plugin.txt"), "not a package").unwrap();

        let error = delete_from_resource_dir(&resource, OsStr::new("plugin")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(resource).unwrap();
    }
}
