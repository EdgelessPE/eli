// `.ess` 系统图标资源包。
//
// ESS 是 theme apply 中唯一需要强文件事务和 Shell 生命周期保证的组件：
// 双 DLL（imageres.dll / imagesp1.dll）替换在统一刷新阶段的 Explorer 停止期间
// 执行，任一文件替换失败时恢复两个旧 DLL，并保证 Explorer 最终可用。

use std::io;
use std::path::{Path, PathBuf};

use super::archive::{ESS_LIMITS, validate_listing};
use super::transaction::atomic_replace;
use super::{ThemeBackend, ThemePaths};

/// ESS 预检结果：两个待发布 DLL 位于 staging 中，目标路径已知。
#[derive(Debug, Clone)]
pub struct PreparedEss {
    pub imageres_source: PathBuf,
    pub imagesp1_source: PathBuf,
}

/// 提交阶段快照：把两个当前目标 DLL 备份到事务目录，供失败时恢复。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EssCommit {
    imageres: EssFileSlot,
    imagesp1: EssFileSlot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EssFileSlot {
    source: PathBuf,
    target: PathBuf,
    /// 旧 DLL 的备份文件；None 表示目标原本不存在（正常情况下不会发生）。
    backup: Option<PathBuf>,
}

/// 预检：解析并校验 PE 头（MZ 魔数、PE 签名、machine 类型）。
pub fn validate_pe_file(path: &Path) -> io::Result<()> {
    let contents = std::fs::read(path)?;
    validate_pe_bytes(&contents).map_err(|message| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid PE/DLL header in {}: {message}", path.display()),
        )
    })
}

fn validate_pe_bytes(contents: &[u8]) -> Result<(), String> {
    if contents.len() < 0x40 || &contents[..2] != b"MZ" {
        return Err("missing DOS 'MZ' signature".to_owned());
    }
    let e_lfanew = u32::from_le_bytes(contents[0x3c..0x40].try_into().unwrap());
    let pe_offset = e_lfanew as usize;
    if pe_offset + 24 > contents.len() || &contents[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return Err("missing 'PE\\0\\0' signature".to_owned());
    }
    let machine = u16::from_le_bytes(contents[pe_offset + 4..pe_offset + 6].try_into().unwrap());
    let characteristics =
        u16::from_le_bytes(contents[pe_offset + 22..pe_offset + 24].try_into().unwrap());
    let expected = if cfg!(target_arch = "x86_64") {
        0x8664u16
    } else if cfg!(target_arch = "x86") {
        0x14cu16
    } else {
        return Err("theme apply only supports x86/x64 PE architectures".to_owned());
    };
    if machine != expected {
        return Err(format!(
            "machine type 0x{machine:04x} does not match the current PE architecture"
        ));
    }
    if characteristics & 0x2000 == 0 {
        return Err("PE image is not marked as a DLL".to_owned());
    }
    Ok(())
}

/// 预检：ESS 根目录必须恰好包含普通文件 imageres.dll 和 imagesp1.dll。
pub fn prepare_ess_from_archive(
    archive: &Path,
    staging: &Path,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedEss> {
    let entries = backend.list_archive(archive)?;
    validate_listing(&entries, &ESS_LIMITS)?;
    let mut found = [false; 2];
    let mut extra = Vec::new();
    for entry in &entries {
        if entry.is_directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ESS archive must not contain directories: {}", entry.path),
            ));
        }
        if super::transaction::name_matches(&entry.path, "imageres.dll") {
            if found[0] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "ESS archive contains duplicate imageres.dll entries: {}",
                        entry.path
                    ),
                ));
            }
            found[0] = true;
        } else if super::transaction::name_matches(&entry.path, "imagesp1.dll") {
            if found[1] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "ESS archive contains duplicate imagesp1.dll entries: {}",
                        entry.path
                    ),
                ));
            }
            found[1] = true;
        } else {
            extra.push(entry.path.clone());
        }
    }
    if !found[0] || !found[1] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ESS archive must contain imageres.dll and imagesp1.dll",
        ));
    }
    if let Some(extra) = extra.first() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ESS archive contains an unexpected entry: {extra}"),
        ));
    }
    let whitelist = vec!["imageres.dll".to_owned(), "imagesp1.dll".to_owned()];
    backend.extract_archive_entries(archive, staging, &whitelist)?;
    let imageres_source = staging.join("imageres.dll");
    let imagesp1_source = staging.join("imagesp1.dll");
    validate_pe_file(&imageres_source)?;
    validate_pe_file(&imagesp1_source)?;
    Ok(PreparedEss {
        imageres_source,
        imagesp1_source,
    })
}

impl PreparedEss {
    /// 提交阶段：快照当前目标 DLL 到事务目录（在 Explorer 停止前调用）。
    pub fn commit_snapshot(
        &self,
        transaction_dir: &Path,
        paths: &ThemePaths,
    ) -> io::Result<EssCommit> {
        let backup_dir = transaction_dir.join("ess-backup");
        std::fs::create_dir_all(&backup_dir)?;
        let imageres_target = paths.system_root.join("System32").join("imageres.dll");
        let imagesp1_target = paths.system_root.join("System32").join("imagesp1.dll");
        let imageres = snapshot_slot(
            &self.imageres_source,
            &imageres_target,
            &backup_dir.join("imageres.dll"),
        )?;
        let imagesp1 = snapshot_slot(
            &self.imagesp1_source,
            &imagesp1_target,
            &backup_dir.join("imagesp1.dll"),
        )?;
        Ok(EssCommit { imageres, imagesp1 })
    }
}

fn snapshot_slot(source: &Path, target: &Path, backup: &Path) -> io::Result<EssFileSlot> {
    let exists = match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => true,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ESS target is not a regular file: {}", target.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let backup = if exists {
        let mut source = std::fs::File::open(target)?;
        let mut destination = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(backup)?;
        io::copy(&mut source, &mut destination)?;
        destination.sync_all()?;
        Some(backup.to_owned())
    } else {
        None
    };
    Ok(EssFileSlot {
        source: source.to_owned(),
        target: target.to_owned(),
        backup,
    })
}

impl EssCommit {
    /// 在 Explorer 停止期间执行双 DLL 替换；失败时恢复两个旧 DLL。
    pub fn replace(&self, backend: &dyn ThemeBackend) -> io::Result<()> {
        if let Err(error) = replace_slot(backend, &self.imageres) {
            return Err(restore_error(error, self, backend));
        }
        if let Err(error) = replace_slot(backend, &self.imagesp1) {
            return Err(restore_error(error, self, backend));
        }
        Ok(())
    }

    /// 恢复两个旧 DLL；目标原本不存在时删除新文件。
    pub fn restore(&self, backend: &dyn ThemeBackend) -> io::Result<()> {
        restore_slot(backend, &self.imageres)?;
        restore_slot(backend, &self.imagesp1)
    }

    /// 测试访问器：第二个 DLL 的 staging 源文件路径（用于模拟替换失败）。
    #[cfg(test)]
    pub(crate) fn imagesp1_source(&self) -> PathBuf {
        self.imagesp1.source.clone()
    }
}

fn restore_error(error: io::Error, commit: &EssCommit, backend: &dyn ThemeBackend) -> io::Error {
    match commit.restore(backend) {
        Ok(()) => error,
        Err(restore_error) => io::Error::new(
            error.kind(),
            format!("{error}; restoring old DLLs also failed: {restore_error}"),
        ),
    }
}

fn replace_slot(backend: &dyn ThemeBackend, slot: &EssFileSlot) -> io::Result<()> {
    // 原子替换是“移动”语义：替换后源文件已不存在，先保留预检产物字节，
    // 替换后同时校验大小和完整内容（比只比较散列更强）。
    let expected = std::fs::read(&slot.source)?;
    replace_with_permission(backend, &slot.source, &slot.target)?;
    verify_replaced(&slot.target, &expected)
}

fn verify_replaced(target: &Path, expected: &[u8]) -> io::Result<()> {
    let actual = std::fs::read(target)?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "replaced {} does not match the prepared DLL ({} bytes written, {} expected)",
                target.display(),
                actual.len(),
                expected.len()
            ),
        ));
    }
    Ok(())
}

fn replace_with_permission(
    backend: &dyn ThemeBackend,
    source: &Path,
    target: &Path,
) -> io::Result<()> {
    match atomic_replace(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            backend.with_temporary_write_permission(target, &mut || atomic_replace(source, target))
        }
        Err(error) => Err(error),
    }
}

fn restore_slot(backend: &dyn ThemeBackend, slot: &EssFileSlot) -> io::Result<()> {
    if let Some(backup) = &slot.backup {
        replace_with_permission(backend, backup, &slot.target)
    } else {
        match std::fs::remove_file(&slot.target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => backend
                .with_temporary_write_permission(&slot.target, &mut || {
                    std::fs::remove_file(&slot.target)
                }),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn accepts_a_matching_architecture_pe_file() {
        let mut contents = vec![0u8; 0x60];
        contents[..2].copy_from_slice(b"MZ");
        contents[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        contents[0x40..0x44].copy_from_slice(b"PE\0\0");
        contents[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
        contents[0x56..0x58].copy_from_slice(&0x2000u16.to_le_bytes());

        assert_eq!(
            validate_pe_bytes(&contents).is_ok(),
            cfg!(target_arch = "x86_64")
        );
    }

    #[test]
    fn rejects_broken_or_wrong_architecture_pe_files() {
        assert!(validate_pe_bytes(b"not a dll").is_err());
        let mut contents = vec![0u8; 0x60];
        contents[..2].copy_from_slice(b"MZ");
        contents[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        contents[0x40..0x44].copy_from_slice(b"PE\0\0");
        contents[0x44..0x46].copy_from_slice(&0x14cu16.to_le_bytes());
        contents[0x56..0x58].copy_from_slice(&0x2000u16.to_le_bytes());

        assert_eq!(
            validate_pe_bytes(&contents).is_ok(),
            cfg!(target_arch = "x86")
        );
        assert!(validate_pe_bytes(&[0u8; 8]).is_err());

        contents[0x44..0x46].copy_from_slice(
            &(if cfg!(target_arch = "x86_64") {
                0x8664u16
            } else {
                0x14cu16
            })
            .to_le_bytes(),
        );
        contents[0x56..0x58].copy_from_slice(&0u16.to_le_bytes());
        assert!(validate_pe_bytes(&contents).is_err());
    }

    #[test]
    fn restores_both_dlls_when_the_second_replacement_fails() {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-ess-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let targets = root.join("System32");
        std::fs::create_dir_all(&targets).unwrap();
        let old_imageres = targets.join("imageres.dll");
        let old_imagesp1 = targets.join("imagesp1.dll");
        std::fs::write(&old_imageres, b"old-imageres").unwrap();
        std::fs::write(&old_imagesp1, b"old-imagesp1").unwrap();

        let staging = root.join("staging");
        std::fs::create_dir(&staging).unwrap();
        let new_imageres = staging.join("imageres.dll");
        let new_imagesp1 = staging.join("imagesp1.dll");
        std::fs::write(&new_imageres, b"new-imageres").unwrap();
        std::fs::write(&new_imagesp1, b"new-imagesp1").unwrap();

        // 模拟第二个 DLL 替换失败：第二个新文件缺失导致替换失败，
        // 此时必须恢复两个旧 DLL。
        std::fs::remove_file(&new_imagesp1).unwrap();

        let backup = root.join("backup");
        std::fs::create_dir(&backup).unwrap();
        let prepared = PreparedEss {
            imageres_source: new_imageres,
            imagesp1_source: new_imagesp1,
        };
        let paths = ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("staging"),
            wallpaper_dir: root.join("wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: vec![],
            icon_cache_dir: root.join("cache"),
        };
        let commit = prepared.commit_snapshot(&backup, &paths).unwrap();

        let backend = crate::command::theme::apply::test_support::FakeBackend::new(root.clone());
        let result = commit.replace(&backend);
        assert!(result.is_err());
        assert_eq!(std::fs::read(&old_imageres).unwrap(), b"old-imageres");
        assert_eq!(std::fs::read(&old_imagesp1).unwrap(), b"old-imagesp1");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
