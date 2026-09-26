use super::ThemeComponent;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(super) const LISTED_COMPONENTS: [ThemeComponent; 6] = [
    ThemeComponent::IconPack,
    ThemeComponent::SystemIconPack,
    ThemeComponent::LoadScreen,
    ThemeComponent::MouseStyle,
    ThemeComponent::StartIsBackConfig,
    ThemeComponent::Wallpaper,
];

pub(super) fn component_relative_path(component: ThemeComponent) -> &'static Path {
    Path::new(match component {
        ThemeComponent::IconPack => "Default/IconPack.eis",
        ThemeComponent::SystemIconPack => "Default/SystemIconPack.ess",
        ThemeComponent::LoadScreen => "Default/LoadScreen",
        ThemeComponent::MouseStyle => "Default/MouseStyle.ems",
        ThemeComponent::StartIsBackConfig => "Default/StartIsBackConfig.esc",
        ThemeComponent::Wallpaper => "wp.jpg",
    })
}

pub(super) fn component_path(edgeless: &Path, component: ThemeComponent) -> PathBuf {
    edgeless.join(component_relative_path(component))
}

pub(super) fn component_is_configured(
    edgeless: &Path,
    component: ThemeComponent,
) -> io::Result<bool> {
    let path = component_path(edgeless, component);
    super::store::ensure_destination(edgeless, &path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("theme resource is a reparse point: {}", path.display()),
        ));
    }
    let expected_type = if component == ThemeComponent::LoadScreen {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    if !expected_type {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("theme resource has an unexpected type: {}", path.display()),
        ));
    }
    Ok(true)
}

#[cfg(windows)]
pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
pub(super) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
