use super::{Plugin, PluginAttribute};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const AUTOMATIC_BUILD_MARKER: &str = "（bot）";

pub(super) fn resource_dir(mount_point: &Path) -> PathBuf {
    mount_point.join("Edgeless").join("Resource")
}

pub(super) fn find(resource_dir: &Path, name: &OsStr) -> io::Result<PathBuf> {
    validate_name(name)?;

    let mut exact_match = None;
    let mut stem_matches = Vec::new();
    for path in paths(resource_dir)? {
        if path.file_name() == Some(name) {
            exact_match = Some(path);
            break;
        }
        if path.file_stem() == Some(name) {
            stem_matches.push(path);
        }
    }

    match exact_match {
        Some(path) => Ok(path),
        None if stem_matches.len() == 1 => Ok(stem_matches.remove(0)),
        None if stem_matches.len() > 1 => {
            let matches = stem_matches
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("plugin name is ambiguous: {matches}"),
            ))
        }
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "plugin package was not found in {}: {}",
                resource_dir.display(),
                name.to_string_lossy()
            ),
        )),
    }
}

pub(super) fn paths(resource_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = fs::read_dir(resource_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read {}: {error}", resource_dir.display()),
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && PluginAttribute::from_path(&entry.path()).is_some() {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

pub(super) fn parse(path: &Path) -> io::Result<Plugin> {
    let attribute = PluginAttribute::from_path(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported plugin package extension: {}", path.display()),
        )
    })?;
    let stem = path.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("plugin package name is not valid UTF-8: {}", path.display()),
        )
    })?;
    let (stem, automatically_built) = match stem.strip_suffix(AUTOMATIC_BUILD_MARKER) {
        Some(stem) => (stem, true),
        None => (stem, false),
    };
    let mut fields = stem.rsplitn(3, '_');
    let author = fields.next().unwrap_or_default();
    let version = fields.next().unwrap_or_default();
    let name = fields.next().unwrap_or_default();
    if name.is_empty() || version.is_empty() || author.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "plugin package name must use <NAME>_<VERSION>_<AUTHOR>[（bot）]: {}",
                path.display()
            ),
        ));
    }

    Ok(Plugin {
        name: name.to_owned(),
        version: version.to_owned(),
        author: author.to_owned(),
        attribute,
        automatically_built,
    })
}

fn validate_name(name: &OsStr) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::test_resource_dir;

    #[test]
    fn rejects_an_ambiguous_stem_across_attribute_variants() {
        let resource = test_resource_dir();
        let normal = resource.join("plugin_1.0_author.7z");
        let frozen = resource.join("plugin_1.0_author.7zf");
        fs::write(&normal, "normal").unwrap();
        fs::write(&frozen, "frozen").unwrap();

        let error = find(&resource, OsStr::new("plugin_1.0_author")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(normal.exists());
        assert!(frozen.exists());
        fs::remove_dir_all(resource).unwrap();
    }
}
