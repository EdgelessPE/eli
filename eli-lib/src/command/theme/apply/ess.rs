// `.ess` 系统图标资源包。
//
// ESS 是 theme apply 中唯一需要强文件事务和 Shell 生命周期保证的组件：
// 双 DLL（imageres.dll / imagesp1.dll）替换在统一刷新阶段的 Explorer 停止期间
// 执行，任一文件替换失败时恢复两个旧 DLL，并保证 Explorer 最终可用。

use std::io;
use std::path::{Path, PathBuf};

use super::archive::{ESS_LIMITS, validate_listing};
use super::transaction::{atomic_replace, ensure_safe_publish_path};
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
    let section_count =
        u16::from_le_bytes(contents[pe_offset + 6..pe_offset + 8].try_into().unwrap()) as usize;
    let optional_size =
        u16::from_le_bytes(contents[pe_offset + 20..pe_offset + 22].try_into().unwrap()) as usize;
    let characteristics =
        u16::from_le_bytes(contents[pe_offset + 22..pe_offset + 24].try_into().unwrap());
    let (expected_machine, expected_magic, minimum_optional_size) =
        if cfg!(all(windows, target_arch = "x86")) {
            (0x14cu16, 0x10bu16, 0xe0usize)
        } else if cfg!(any(
            all(windows, target_arch = "x86_64"),
            all(test, not(windows))
        )) {
            // 非 Windows 宿主上的 fake 后端统一模拟当前主流的 x64 PE，避免把
            // macOS ARM64 宿主架构错误映射成不存在的 Windows ARM64 主题包。
            (0x8664u16, 0x20bu16, 0xf0usize)
        } else {
            return Err("theme apply only supports x86/x64 PE architectures".to_owned());
        };
    if machine != expected_machine {
        return Err(format!(
            "machine type 0x{machine:04x} does not match the current PE architecture"
        ));
    }
    if characteristics & 0x2000 == 0 {
        return Err("PE image is not marked as a DLL".to_owned());
    }
    if section_count == 0 || section_count > 96 {
        return Err(format!("invalid PE section count: {section_count}"));
    }
    if optional_size < minimum_optional_size {
        return Err(format!(
            "PE optional header is too small ({optional_size} bytes; expected at least {minimum_optional_size})"
        ));
    }
    let optional_start = pe_offset + 24;
    let optional_end = optional_start
        .checked_add(optional_size)
        .ok_or_else(|| "PE optional header range overflows".to_owned())?;
    let section_table_end = optional_end
        .checked_add(
            section_count
                .checked_mul(40)
                .ok_or_else(|| "PE section table size overflows".to_owned())?,
        )
        .ok_or_else(|| "PE section table range overflows".to_owned())?;
    if section_table_end > contents.len() {
        return Err("PE optional header or section table is truncated".to_owned());
    }
    let magic = u16::from_le_bytes(
        contents[optional_start..optional_start + 2]
            .try_into()
            .unwrap(),
    );
    if magic != expected_magic {
        return Err(format!(
            "PE optional header magic 0x{magic:04x} does not match the current architecture"
        ));
    }
    let size_of_headers = u32::from_le_bytes(
        contents[optional_start + 60..optional_start + 64]
            .try_into()
            .unwrap(),
    ) as usize;
    if size_of_headers < section_table_end || size_of_headers > contents.len() {
        return Err(format!("invalid PE SizeOfHeaders value: {size_of_headers}"));
    }
    for section in 0..section_count {
        let offset = optional_end + section * 40;
        let raw_size = u32::from_le_bytes(contents[offset + 16..offset + 20].try_into().unwrap());
        let raw_pointer =
            u32::from_le_bytes(contents[offset + 20..offset + 24].try_into().unwrap());
        if raw_size == 0 {
            continue;
        }
        let raw_end = (raw_pointer as usize)
            .checked_add(raw_size as usize)
            .ok_or_else(|| format!("PE section {} raw range overflows", section + 1))?;
        if (raw_pointer as usize) < size_of_headers || raw_end > contents.len() {
            return Err(format!(
                "PE section {} raw data is outside the file",
                section + 1
            ));
        }
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
    let mut found: [Option<String>; 2] = [None, None];
    let mut extra = Vec::new();
    for entry in &entries {
        if entry.is_directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ESS archive must not contain directories: {}", entry.path),
            ));
        }
        if super::transaction::name_matches(&entry.path, "imageres.dll") {
            if found[0].is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "ESS archive contains duplicate imageres.dll entries: {}",
                        entry.path
                    ),
                ));
            }
            found[0] = Some(entry.path.clone());
        } else if super::transaction::name_matches(&entry.path, "imagesp1.dll") {
            if found[1].is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "ESS archive contains duplicate imagesp1.dll entries: {}",
                        entry.path
                    ),
                ));
            }
            found[1] = Some(entry.path.clone());
        } else {
            extra.push(entry.path.clone());
        }
    }
    if found.iter().any(Option::is_none) {
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
    let whitelist = found
        .iter()
        .map(|name| name.clone().expect("both ESS entries were checked above"))
        .collect::<Vec<_>>();
    backend.extract_archive_entries(archive, staging, &whitelist)?;
    let imageres_source = staging.join(&whitelist[0]);
    let imagesp1_source = staging.join(&whitelist[1]);
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
        ensure_safe_publish_path(&paths.system_root, &imageres_target)?;
        ensure_safe_publish_path(&paths.system_root, &imagesp1_target)?;
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
            return Err(restore_error(
                io::Error::new(error.kind(), format!("replace imageres.dll: {error}")),
                self,
                backend,
            ));
        }
        if let Err(error) = replace_slot(backend, &self.imagesp1) {
            return Err(restore_error(
                io::Error::new(error.kind(), format!("replace imagesp1.dll: {error}")),
                self,
                backend,
            ));
        }
        Ok(())
    }

    /// 恢复两个旧 DLL；目标原本不存在时删除新文件。
    pub fn restore(&self, backend: &dyn ThemeBackend) -> io::Result<()> {
        let mut failures = Vec::new();
        if let Err(error) = restore_slot(backend, &self.imageres) {
            failures.push(format!("restore imageres.dll: {error}"));
        }
        if let Err(error) = restore_slot(backend, &self.imagesp1) {
            failures.push(format!("restore imagesp1.dll: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
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
    let target_root = slot.target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ESS target has no parent: {}", slot.target.display()),
        )
    })?;
    ensure_safe_publish_path(target_root, &slot.target)?;
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
            backend
                .with_temporary_write_permission(target, &mut || atomic_replace(source, target))
                .map_err(|permission_error| {
                    io::Error::new(
                        permission_error.kind(),
                        format!(
                            "temporary permission fallback failed for {} after the initial replace was denied: {permission_error}",
                            target.display()
                        ),
                    )
                })
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
    use crate::command::theme::apply::details::archive::ArchiveEntry;
    use crate::command::theme::apply::test_support::{FakeArchive, FakeBackend, test_root};
    use std::collections::HashMap;

    fn matching_pe_bytes() -> Vec<u8> {
        let mut contents = vec![0u8; 1024];
        contents[..2].copy_from_slice(b"MZ");
        contents[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        contents[0x80..0x84].copy_from_slice(b"PE\0\0");
        let (machine, magic, optional_size) = if cfg!(all(windows, target_arch = "x86")) {
            (0x14cu16, 0x10bu16, 0xe0u16)
        } else {
            (0x8664u16, 0x20bu16, 0xf0u16)
        };
        contents[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        contents[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        contents[0x94..0x96].copy_from_slice(&optional_size.to_le_bytes());
        contents[0x96..0x98].copy_from_slice(&0x2000u16.to_le_bytes());
        let optional_start = 0x98usize;
        contents[optional_start..optional_start + 2].copy_from_slice(&magic.to_le_bytes());
        contents[optional_start + 60..optional_start + 64].copy_from_slice(&512u32.to_le_bytes());
        let section = optional_start + optional_size as usize;
        contents[section..section + 5].copy_from_slice(b".rsrc");
        contents[section + 16..section + 20].copy_from_slice(&512u32.to_le_bytes());
        contents[section + 20..section + 24].copy_from_slice(&512u32.to_le_bytes());
        contents
    }

    #[test]
    fn accepts_a_matching_architecture_pe_file() {
        assert!(validate_pe_bytes(&matching_pe_bytes()).is_ok());
    }

    #[test]
    fn rejects_broken_or_wrong_architecture_pe_files() {
        assert!(validate_pe_bytes(b"not a dll").is_err());
        let mut contents = matching_pe_bytes();
        let wrong_machine = if cfg!(all(windows, target_arch = "x86")) {
            0x8664u16
        } else {
            0x14cu16
        };
        contents[0x84..0x86].copy_from_slice(&wrong_machine.to_le_bytes());
        assert!(validate_pe_bytes(&contents).is_err());
        assert!(validate_pe_bytes(&[0u8; 8]).is_err());

        contents = matching_pe_bytes();
        contents[0x96..0x98].copy_from_slice(&0u16.to_le_bytes());
        assert!(validate_pe_bytes(&contents).is_err());

        let mut truncated = vec![0u8; 96];
        truncated[..2].copy_from_slice(b"MZ");
        truncated[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        truncated[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert!(validate_pe_bytes(&truncated).is_err());
    }

    #[test]
    fn preserves_actual_entry_case_when_extracting_ess_files() {
        let root = test_root("ess-case");
        let archive = root.join("icons.ess");
        std::fs::write(&archive, b"fake archive").unwrap();
        let backend = FakeBackend::new(root.clone());
        let entries = ["ImageRes.DLL", "IMAGESP1.dll"];
        backend.state.lock().unwrap().archives.insert(
            archive.clone(),
            FakeArchive {
                entries: entries
                    .iter()
                    .map(|path| ArchiveEntry {
                        path: (*path).to_owned(),
                        size: 96,
                        is_directory: false,
                        encrypted: false,
                        is_link: false,
                    })
                    .collect(),
                files: entries
                    .iter()
                    .map(|path| ((*path).to_owned(), matching_pe_bytes()))
                    .collect::<HashMap<_, _>>(),
            },
        );

        let prepared = prepare_ess_from_archive(&archive, &root.join("staging"), &backend).unwrap();

        assert_eq!(
            prepared.imageres_source.file_name().unwrap(),
            "ImageRes.DLL"
        );
        assert_eq!(
            prepared.imagesp1_source.file_name().unwrap(),
            "IMAGESP1.dll"
        );
        let state = backend.state.lock().unwrap();
        assert_eq!(
            state.extraction_whitelists.last().unwrap(),
            &["ImageRes.DLL".to_owned(), "IMAGESP1.dll".to_owned()]
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_both_dlls_when_the_second_replacement_fails() {
        let root = test_root("ess-replace");
        let targets = root.join("Windows/System32");
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

    #[test]
    fn attempts_both_dll_restores_when_the_first_restore_fails() {
        let root = test_root("ess-restore-all");
        let first_target = root.join("imageres.dll");
        let second_target = root.join("imagesp1.dll");
        let second_backup = root.join("imagesp1.backup.dll");
        std::fs::write(&first_target, b"new-first").unwrap();
        std::fs::write(&second_target, b"new-second").unwrap();
        std::fs::write(&second_backup, b"old-second").unwrap();
        let commit = EssCommit {
            imageres: EssFileSlot {
                source: root.join("unused-first-source"),
                target: first_target,
                backup: Some(root.join("missing-first-backup")),
            },
            imagesp1: EssFileSlot {
                source: root.join("unused-second-source"),
                target: second_target.clone(),
                backup: Some(second_backup),
            },
        };
        let backend = FakeBackend::new(root.clone());

        let error = commit.restore(&backend).unwrap_err();

        assert!(error.to_string().contains("restore imageres.dll"));
        assert_eq!(std::fs::read(second_target).unwrap(), b"old-second");
        std::fs::remove_dir_all(root).unwrap();
    }
}
