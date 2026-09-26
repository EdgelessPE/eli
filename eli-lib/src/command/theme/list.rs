use super::ThemeComponent;
use super::storage::{LISTED_COMPONENTS, component_is_configured};
use crate::Ctx;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedThemeResource {
    pub resource: ThemeComponent,
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeListSummary {
    pub bootdisk: PathBuf,
    pub resources: Vec<ListedThemeResource>,
}

/// 列出所选启动盘中实际存在的主题资源。
pub fn list(ctx: &Ctx) -> io::Result<ThemeListSummary> {
    let bootdisk = &ctx.bootdisk()?.selected;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    let edgeless = bootdisk.mount_point.join("Edgeless");
    let resources = list_at(&edgeless)?;
    Ok(ThemeListSummary {
        bootdisk: bootdisk.mount_point.clone(),
        resources,
    })
}

fn list_at(edgeless: &Path) -> io::Result<Vec<ListedThemeResource>> {
    let metadata = fs::symlink_metadata(edgeless)?;
    if !metadata.is_dir() || super::storage::is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Edgeless path is not a safe directory: {}",
                edgeless.display()
            ),
        ));
    }
    LISTED_COMPONENTS
        .into_iter()
        .map(|resource| {
            Ok(ListedThemeResource {
                resource,
                configured: component_is_configured(edgeless, resource)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lists_only_actual_fixed_theme_resources() {
        let root = test_root("configured");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default/LoadScreen")).unwrap();
        fs::write(edgeless.join("Default/IconPack.eis"), b"icons").unwrap();
        fs::write(edgeless.join("Default/Info.txt"), b"stale metadata").unwrap();
        fs::write(edgeless.join("wp.jpg"), b"wallpaper").unwrap();

        let resources = list_at(&edgeless).unwrap();

        assert_eq!(resources.len(), LISTED_COMPONENTS.len());
        assert!(configured(&resources, ThemeComponent::IconPack));
        assert!(configured(&resources, ThemeComponent::LoadScreen));
        assert!(configured(&resources, ThemeComponent::Wallpaper));
        assert!(!configured(&resources, ThemeComponent::SystemIconPack));
        assert!(!configured(&resources, ThemeComponent::MouseStyle));
        assert!(!configured(&resources, ThemeComponent::StartIsBackConfig));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_follow_a_resource_symbolic_link() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlink");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default")).unwrap();
        let outside = root.join("outside.eis");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, edgeless.join("Default/IconPack.eis")).unwrap();

        let error = list_at(&edgeless).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(root).unwrap();
    }

    fn configured(resources: &[ListedThemeResource], resource: ThemeComponent) -> bool {
        resources
            .iter()
            .find(|entry| entry.resource == resource)
            .is_some_and(|entry| entry.configured)
    }

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-list-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
