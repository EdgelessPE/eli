use super::{BOOLEAN_CONFIGS, HOMEPAGE_HIGHER_THAN, parse_bootdisk_version};
use crate::Ctx;
use std::fs;
use std::io;
use std::path::Path;

/// 启动盘配置项的当前状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
    pub available: bool,
}

/// 列出选中启动盘上的全部可管理配置。
pub fn list(ctx: &Ctx) -> io::Result<Vec<ConfigEntry>> {
    let bootdisk = &ctx.bootdisk()?.selected;
    list_in_edgeless_dir(&bootdisk.mount_point.join("Edgeless"), &bootdisk.version)
}

fn list_in_edgeless_dir(edgeless_dir: &Path, version: &str) -> io::Result<Vec<ConfigEntry>> {
    let version = parse_bootdisk_version(version)?;
    let config_dir = edgeless_dir.join("Config");
    let mut entries = Vec::with_capacity(BOOLEAN_CONFIGS.len() + 3);
    for config in BOOLEAN_CONFIGS {
        entries.push(ConfigEntry {
            key: config.key.to_owned(),
            value: marker_enabled(&config_dir.join(config.key))?.to_string(),
            available: config.is_available_for(version),
        });
    }
    entries.push(ConfigEntry {
        key: "wallpaper".to_owned(),
        value: if edgeless_dir.join("wp.jpg").is_file() {
            "set"
        } else {
            "not set"
        }
        .to_owned(),
        available: true,
    });
    entries.push(ConfigEntry {
        key: "resolution".to_owned(),
        value: read_optional_text(&config_dir.join("分辨率.txt"))?
            .unwrap_or_else(|| "auto".to_owned()),
        available: true,
    });
    entries.push(ConfigEntry {
        key: "homepage".to_owned(),
        value: read_optional_text(&config_dir.join("HomePage.txt"))?
            .unwrap_or_else(|| "not set".to_owned()),
        available: version > HOMEPAGE_HIGHER_THAN,
    });
    Ok(entries)
}

pub(super) fn marker_enabled(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("config marker is not a directory: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_optional_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reports_boolean_state_and_version_availability() {
        let root = std::env::temp_dir().join(format!(
            "eli-lib-config-list-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("Config");
        fs::create_dir_all(config.join("DisablePinBrowsers")).unwrap();
        let entries = list_in_edgeless_dir(&root, "Edgeless_Beta_Ofial_4.1.0_2").unwrap();
        assert!(entries.iter().any(|entry| entry.key == "DisablePinBrowsers"
            && entry.value == "true"
            && entry.available));
        assert!(
            entries
                .iter()
                .any(|entry| entry.key == "Developer" && !entry.available)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.key == "homepage" && entry.available)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
