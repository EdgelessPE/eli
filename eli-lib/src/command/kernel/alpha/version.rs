use super::super::is_reparse_point;
use crate::Ctx;
use crate::api::edgeless;
use crate::version_identifier::{EdgelessVersionIdentifier, ReleaseStage};
use std::fs;
use std::io;
use std::path::Path;

/// 返回网络上可用的最新 Alpha 内核版本标识符。
pub fn latest(token: &str) -> io::Result<EdgelessVersionIdentifier> {
    edgeless::latest_alpha_version(token)
}

/// 返回选中启动盘中保存的最高 Alpha 内核版本标识符。
///
/// Alpha 内核以 `Edgeless_Alpha_*.wim` 形式直接保存在启动盘根目录。扫描只读取
/// 常规文件且不跟随重解析点，多个版本并存时按版本号选择最高项。
pub fn bootdisk(ctx: &Ctx) -> io::Result<EdgelessVersionIdentifier> {
    latest_alpha_version_in_directory(&ctx.bootdisk()?.selected.mount_point)
}

fn latest_alpha_version_in_directory(directory: &Path) -> io::Result<EdgelessVersionIdentifier> {
    let mut latest = None;
    let entries = fs::read_dir(directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to inspect Alpha kernels on boot disk {}: {error}",
                directory.display()
            ),
        )
    })?;

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let path = entry.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wim"))
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !stem.starts_with("Edgeless_Alpha_") {
            continue;
        }
        let Ok(version) = EdgelessVersionIdentifier::parse(stem) else {
            continue;
        };
        if version.stage != ReleaseStage::Alpha || version.to_string() != stem {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || is_reparse_point(&metadata) {
            continue;
        }
        if latest
            .is_none_or(|previous: EdgelessVersionIdentifier| version.version > previous.version)
        {
            latest = Some(version);
        }
    }

    latest.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no Edgeless Alpha kernel WIM found on boot disk {}",
                directory.display()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn selects_the_highest_alpha_wim_version() {
        let directory = temporary_directory();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("Edgeless_Alpha_4.1.2.wim"), b"old").unwrap();
        fs::write(directory.join("Edgeless_Alpha_4.2.0.wim"), b"latest").unwrap();
        fs::write(directory.join("Edgeless_Beta_4.9.0.wim"), b"beta").unwrap();
        fs::write(directory.join("Edgeless_Alpa_9.0.0.wim"), b"typo").unwrap();

        let version = latest_alpha_version_in_directory(&directory).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Alpha_4.2.0");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_a_bootdisk_without_an_alpha_wim() {
        let directory = temporary_directory();
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("Edgeless_Beta_4.1.0.wim"), b"beta").unwrap();

        let error = latest_alpha_version_in_directory(&directory).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(directory).unwrap();
    }

    fn temporary_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eli-kernel-alpha-version-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
