use crate::Ctx;
use crate::api::edgeless;
use crate::dependency::RuntimeEnvironment;
use crate::version_identifier::EdgelessVersionIdentifier;
use std::fs;
use std::io;
use std::path::Path;

const CURRENT_VERSION_PATH: &str = r"X:\Program Files\version.txt";

/// 返回当前 PE 内核的版本标识符。
pub fn current(ctx: &Ctx) -> io::Result<EdgelessVersionIdentifier> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;
    read_version_file(Path::new(CURRENT_VERSION_PATH))
}

/// 返回网络上可用的最新 Beta 内核版本标识符。
pub fn latest() -> io::Result<EdgelessVersionIdentifier> {
    edgeless::latest_beta_version()
}

/// 返回选中启动盘中保存的内核版本标识符。
///
/// `Ctx::bootdisk` 通过启动盘发现和选择入口取得并缓存唯一选中项；这里重新读取
/// 版本文件，以便命令结果始终反映执行时磁盘上的内容。
pub fn bootdisk(ctx: &Ctx) -> io::Result<EdgelessVersionIdentifier> {
    let bootdisk = &ctx.bootdisk()?.selected;
    read_version_file(&bootdisk.mount_point.join("Edgeless").join("version.txt"))
}

fn read_version_file(path: &Path) -> io::Result<EdgelessVersionIdentifier> {
    let content = fs::read_to_string(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read {}: {error}", path.display()),
        )
    })?;
    let value = content.trim();
    value.parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid Edgeless version identifier in {}: {error}",
                path.display()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_and_normalizes_a_version_file() {
        let path = temporary_path();
        fs::write(&path, "Edgeless_Alpa_4.1.2\r\n").unwrap();

        let version = read_version_file(&path).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Alpha_4.1.2");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_an_invalid_version_file() {
        let path = temporary_path();
        fs::write(&path, "not-a-version").unwrap();

        let error = read_version_file(&path).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains("invalid Edgeless version identifier")
        );
        fs::remove_file(path).unwrap();
    }

    fn temporary_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eli-kernel-version-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
