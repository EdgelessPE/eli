use super::ThemeComponent;
use super::storage::{LISTED_COMPONENTS, component_is_configured, component_path};
use super::store::{
    PublishChange, StagingDirectory, ensure_real_directory, publish_plan, read_info,
    recover_interrupted_transactions, write_info,
};
use crate::Ctx;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeDeleteTarget {
    Resource(ThemeComponent),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDeleteSummary {
    pub bootdisk: PathBuf,
    pub deleted: Vec<ThemeComponent>,
}

/// 从所选启动盘删除一个或全部已配置的主题资源。
pub fn delete(ctx: &Ctx, target: ThemeDeleteTarget) -> io::Result<ThemeDeleteSummary> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    let edgeless = bootdisk.mount_point.join("Edgeless");
    let deleted = delete_at(&edgeless, target)?;
    Ok(ThemeDeleteSummary {
        bootdisk: bootdisk.mount_point.clone(),
        deleted,
    })
}

fn delete_at(edgeless: &Path, target: ThemeDeleteTarget) -> io::Result<Vec<ThemeComponent>> {
    ensure_real_directory(edgeless)?;
    recover_interrupted_transactions(edgeless)?;
    let requested = match target {
        ThemeDeleteTarget::Resource(resource) => vec![resource],
        ThemeDeleteTarget::All => LISTED_COMPONENTS.to_vec(),
    };
    let mut deleted = Vec::new();
    for resource in requested {
        if component_is_configured(edgeless, resource)? {
            deleted.push(resource);
        }
    }
    if deleted.is_empty()
        && let ThemeDeleteTarget::Resource(resource) = target
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "theme resource is not configured: {}",
                resource.display_name()
            ),
        ));
    }

    let metadata_components = match target {
        ThemeDeleteTarget::All => LISTED_COMPONENTS
            .into_iter()
            .filter(|component| *component != ThemeComponent::Wallpaper)
            .collect::<Vec<_>>(),
        ThemeDeleteTarget::Resource(resource) if resource != ThemeComponent::Wallpaper => {
            vec![resource]
        }
        ThemeDeleteTarget::Resource(_) => Vec::new(),
    };
    let info_path = edgeless.join("Default/Info.txt");
    let metadata_change = if metadata_components.is_empty() {
        None
    } else {
        let original = read_info(&info_path)?;
        let mut updated = original.clone();
        for component in metadata_components {
            updated.clear(component);
        }
        (updated != original).then_some(updated)
    };
    if deleted.is_empty() && metadata_change.is_none() {
        return Ok(Vec::new());
    }

    let staging = StagingDirectory::new(edgeless)?;
    let mut changes = deleted
        .iter()
        .map(|resource| PublishChange {
            staged: None,
            destination: component_path(edgeless, *resource),
        })
        .collect::<Vec<_>>();
    if let Some(info) = metadata_change {
        let staged_info = staging.path.join("Info.txt");
        write_info(&staged_info, &info)?;
        changes.push(PublishChange {
            staged: Some(staged_info),
            destination: info_path,
        });
    }
    publish_plan(edgeless, &staging, &changes)?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::GBK;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn deletes_one_resource_and_resets_its_metadata() {
        let root = test_root("one");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default")).unwrap();
        fs::write(edgeless.join("Default/IconPack.eis"), b"icons").unwrap();
        let mut info = super::super::store::ThemeInfo::default();
        info.set(ThemeComponent::IconPack, "OldIcons");
        super::super::store::write_info(&edgeless.join("Default/Info.txt"), &info).unwrap();

        let deleted = delete_at(
            &edgeless,
            ThemeDeleteTarget::Resource(ThemeComponent::IconPack),
        )
        .unwrap();

        assert_eq!(deleted, vec![ThemeComponent::IconPack]);
        assert!(!edgeless.join("Default/IconPack.eis").exists());
        let bytes = fs::read(edgeless.join("Default/Info.txt")).unwrap();
        assert!(GBK.decode(&bytes).0.contains("图标资源包：Unknown "));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_all_removes_only_active_resources_and_preserves_wallpaper_backup() {
        let root = test_root("all");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default/LoadScreen")).unwrap();
        fs::write(edgeless.join("Default/MouseStyle.ems"), b"mouse").unwrap();
        fs::write(edgeless.join("wp.jpg"), b"current").unwrap();
        fs::write(edgeless.join("wp_backup.jpg"), b"backup").unwrap();

        let deleted = delete_at(&edgeless, ThemeDeleteTarget::All).unwrap();

        assert_eq!(
            deleted,
            vec![
                ThemeComponent::LoadScreen,
                ThemeComponent::MouseStyle,
                ThemeComponent::Wallpaper,
            ]
        );
        assert!(!edgeless.join("Default/LoadScreen").exists());
        assert!(!edgeless.join("Default/MouseStyle.ems").exists());
        assert!(!edgeless.join("wp.jpg").exists());
        assert_eq!(fs::read(edgeless.join("wp_backup.jpg")).unwrap(), b"backup");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_a_missing_resource_is_an_error_but_all_is_idempotent() {
        let root = test_root("missing");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(&edgeless).unwrap();

        let error = delete_at(
            &edgeless,
            ThemeDeleteTarget::Resource(ThemeComponent::SystemIconPack),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(
            delete_at(&edgeless, ThemeDeleteTarget::All)
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_all_resets_stale_metadata_without_resource_files() {
        let root = test_root("stale-info");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default")).unwrap();
        let mut info = super::super::store::ThemeInfo::default();
        info.set(ThemeComponent::LoadScreen, "Legacy");
        info.set(ThemeComponent::StartIsBackConfig, "OldMenu");
        super::super::store::write_info(&edgeless.join("Default/Info.txt"), &info).unwrap();

        let deleted = delete_at(&edgeless, ThemeDeleteTarget::All).unwrap();

        assert!(deleted.is_empty());
        let bytes = fs::read(edgeless.join("Default/Info.txt")).unwrap();
        let text = GBK.decode(&bytes).0;
        assert!(text.contains("LoadScreen资源包：Unknown "));
        assert!(text.contains("开始菜单样式配置文件：Unknown "));
        assert!(!text.contains("Legacy"));
        assert!(!text.contains("OldMenu"));
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-delete-{name}-{}-{}",
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
