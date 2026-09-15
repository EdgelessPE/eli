// 统一安全归档层。
//
// 所有由 `theme apply` 实际打开的 7z 格式包都经过同一流程：列出条目 → 校验
// （路径穿越、链接、加密、资源上限、Windows 不区分大小写碰撞）→ 白名单解压
// → 解压后再次遍历确认仍在 staging 根目录内。被跳过的 ELS 不进入该流程。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use super::transaction::{case_fold, path_is_within};

/// 单个 7z 条目的技术信息（解析自 `7z l -slt -sccUTF-8` 输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// 归档内相对路径（7z 使用 `/` 分隔）。
    pub path: String,
    pub size: u64,
    pub is_directory: bool,
    pub encrypted: bool,
    pub is_link: bool,
}

/// 资源上限（SDD 第 8 节）。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_entries: usize,
    pub max_total_size: u64,
}

pub const ETH_LIMITS: Limits = Limits {
    max_entries: 64,
    max_total_size: 1 << 30,
};
pub const EIS_LIMITS: Limits = Limits {
    max_entries: 4096,
    max_total_size: 512 << 20,
};
pub const EMS_LIMITS: Limits = Limits {
    max_entries: 64,
    max_total_size: 128 << 20,
};
pub const ESS_LIMITS: Limits = Limits {
    max_entries: 8,
    max_total_size: 256 << 20,
};

/// 设备名（Windows 保留名）。带扩展名同样视为非法。
fn is_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = case_fold(stem);
    let basic = ["CON", "PRN", "AUX", "NUL", "CLOCK$"];
    if basic.contains(&upper.as_str()) {
        return true;
    }
    for prefix in ["CON", "COM", "LPT"] {
        if upper.len() == 4
            && upper.starts_with(prefix)
            && let Some(digit) = upper.as_bytes().get(3)
            && (b'1'..=b'9').contains(digit)
        {
            return true;
        }
    }
    false
}

/// 校验单个条目路径：拒绝绝对路径、盘符、UNC、`..`、NTFS ADS、设备名和空文件名。
fn validate_entry_path(path: &str) -> io::Result<()> {
    let message = |reason: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsafe archive entry path `{path}` (an entry {reason})"),
        )
    };
    // 目录条目通常以 `/` 结尾（7z 的 `Path = dir/`）；先去掉结尾斜杠再校验。
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(message("with an empty name"));
    }
    if trimmed.contains('\\') {
        return Err(message("using a backslash path separator"));
    }
    for component in trimmed.split('/') {
        if component.is_empty() {
            return Err(message("containing an empty path component"));
        }
        if component == "." {
            return Err(message("containing a dot path component"));
        }
        if component == ".." {
            return Err(message("containing a parent directory component"));
        }
        if component.contains(':') {
            return Err(message(
                "containing a drive letter or alternate data stream",
            ));
        }
        if component.ends_with([' ', '.']) {
            return Err(message(
                "ending a path component with a space or dot, which aliases another Windows path",
            ));
        }
        if is_device_name(component) {
            return Err(message("using a reserved device name"));
        }
    }
    Ok(())
}

/// 对整个清单执行安全基线校验（路径、链接、加密、碰撞、资源上限）。
pub fn validate_listing(entries: &[ArchiveEntry], limits: &Limits) -> io::Result<()> {
    if entries.len() > limits.max_entries {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "archive contains {} entries, exceeding the limit of {}",
                entries.len(),
                limits.max_entries
            ),
        ));
    }
    let total_size = entries
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.size));
    if total_size > limits.max_total_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "archive expands to {total_size} bytes, exceeding the limit of {}",
                limits.max_total_size
            ),
        ));
    }
    let mut folded_paths = HashMap::<String, &str>::new();
    for entry in entries {
        validate_entry_path(&entry.path)?;
        if entry.is_link {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "archive contains a symbolic/hard link or junction entry: {}",
                    entry.path
                ),
            ));
        }
        if entry.encrypted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("archive contains an encrypted entry: {}", entry.path),
            ));
        }
        // Windows 将显式目录条目 `name/` 与同名文件 `name` 解析到同一路径。
        let folded = case_fold(entry.path.trim_end_matches('/'));
        if let Some(previous) = folded_paths.insert(folded.clone(), &entry.path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "archive contains case-insensitive collision on Windows: `{previous}` vs `{}`",
                    entry.path
                ),
            ));
        }
    }
    Ok(())
}

/// 解析 `7z l -slt -sccUTF-8` 的标准输出。
pub fn parse_listing(output: &str) -> io::Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    let mut current: Option<(String, u64, bool, bool, bool)> = None;
    let has_entry_separator = output.lines().any(|line| {
        let line = line.trim();
        line.len() >= 5 && line.bytes().all(|byte| byte == b'-')
    });
    let mut in_entries = !has_entry_separator;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 5 && trimmed.bytes().all(|byte| byte == b'-') {
            current = None;
            in_entries = true;
            continue;
        }
        if !in_entries {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Path = ") {
            if let Some(entry) = current.take() {
                entries.push(to_entry(entry));
            }
            current = Some((rest.to_owned(), 0, false, false, false));
        } else if let Some((_candidate, size, is_directory, encrypted, is_link)) = current.as_mut()
        {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "Size" => {
                    *size = value.parse().map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("7-Zip returned an invalid entry size `{value}`: {error}"),
                        )
                    })?;
                }
                "Attributes" if value.contains('D') => *is_directory = true,
                "Encrypted" if value == "+" => *encrypted = true,
                "SymLink" | "Symbolic Link" | "HardLink" | "Hard Link" | "Junction"
                    if !value.is_empty() =>
                {
                    *is_link = true;
                }
                "Reparse" | "Reparse Point" if !value.is_empty() => *is_link = true,
                _ => {}
            }
        }
    }
    if let Some(entry) = current.take() {
        entries.push(to_entry(entry));
    }
    Ok(entries)
}

fn to_entry(raw: (String, u64, bool, bool, bool)) -> ArchiveEntry {
    ArchiveEntry {
        path: raw.0,
        size: raw.1,
        is_directory: raw.2,
        encrypted: raw.3,
        is_link: raw.4,
    }
}

/// 解压后确认每个解析路径仍位于 staging 根目录内，且没有链接/重解析点。
pub fn verify_extraction(staging_root: &Path) -> io::Result<()> {
    let mut pending = vec![staging_root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if !path_is_within(staging_root, &path) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "extracted entry escaped the staging directory: {}",
                        path.display()
                    ),
                ));
            }
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse_point(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "extracted entry is a symbolic link or reparse point: {}",
                        path.display()
                    ),
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_technical_listing_with_multiple_entries() {
        let output = "\
7-Zip 24.09 (x64) : Copyright (c) 1999-2024 Igor Pavlov : 2024-11-29

Scanning the drive for archives:
1 file, 100 bytes

Listing archive: sample.eth

----------
Path = WallPaper.jpg
Size = 42
Packed Size = 40
Modified = 2025-01-01 00:00:00
Attributes = A
Encrypted = 
Method = LZMA2:24
Block = 0
CRC = 12345678

Path = LoadScreen.els/
Size = 0
Packed Size = 0
Modified = 2025-01-01 00:00:00
Attributes = D
Encrypted = 
Method = 
Block = 0
CRC = 

";
        let entries = parse_listing(output).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "WallPaper.jpg");
        assert_eq!(entries[0].size, 42);
        assert!(!entries[0].is_directory);
        assert!(!entries[0].encrypted);
        assert!(entries[1].is_directory);
        assert_eq!(entries[1].path, "LoadScreen.els/");
    }

    #[test]
    fn parses_encrypted_and_link_markers() {
        let output = "\
Path = secret.txt
Size = 1
Attributes = A
Encrypted = +
Method = AES256

Path = outside.exe
Size = 2
Attributes = A
SymLink = +

";
        let entries = parse_listing(output).unwrap();

        assert!(entries[0].encrypted);
        assert!(entries[1].is_link);
    }

    #[test]
    fn rejects_unsafe_entry_paths() {
        for path in [
            "/absolute.txt",
            "C:/drive.txt",
            "//server/share.txt",
            "../escape.txt",
            "dir/../escape.txt",
            "file:ads.txt",
            "file.txt:ads",
            "CON.txt",
            "COM1.exe",
            "",
            "dir//double.txt",
        ] {
            if validate_entry_path(path).is_ok() {
                panic!("unsafe path `{path}` was accepted");
            }
        }
        assert!(validate_entry_path("dir/file.txt").is_ok());
    }

    #[test]
    fn rejects_case_insensitive_collisions_on_windows() {
        let entries = vec![
            ArchiveEntry {
                path: "Icon-1.ico".to_owned(),
                size: 1,
                is_directory: false,
                encrypted: false,
                is_link: false,
            },
            ArchiveEntry {
                path: "ICON-1.ICO".to_owned(),
                size: 1,
                is_directory: false,
                encrypted: false,
                is_link: false,
            },
        ];

        let error = validate_listing(&entries, &EMS_LIMITS).unwrap_err();

        assert!(error.to_string().contains("collision"));

        let directory_and_file = vec![
            ArchiveEntry {
                path: "shortcut/".to_owned(),
                size: 0,
                is_directory: true,
                encrypted: false,
                is_link: false,
            },
            ArchiveEntry {
                path: "SHORTCUT".to_owned(),
                size: 1,
                is_directory: false,
                encrypted: false,
                is_link: false,
            },
        ];
        assert!(
            validate_listing(&directory_and_file, &EMS_LIMITS)
                .unwrap_err()
                .to_string()
                .contains("collision")
        );
    }

    #[test]
    fn rejects_archives_beyond_the_resource_limits() {
        let many = (0..=64)
            .map(|index| ArchiveEntry {
                path: format!("file-{index}.txt"),
                size: 0,
                is_directory: false,
                encrypted: false,
                is_link: false,
            })
            .collect::<Vec<_>>();
        assert!(validate_listing(&many, &ETH_LIMITS).is_err());

        let huge = vec![ArchiveEntry {
            path: "huge.bin".to_owned(),
            size: ETH_LIMITS.max_total_size + 1,
            is_directory: false,
            encrypted: false,
            is_link: false,
        }];
        assert!(validate_listing(&huge, &ETH_LIMITS).is_err());

        let encrypted = vec![ArchiveEntry {
            path: "secret.7z".to_owned(),
            size: 1,
            is_directory: false,
            encrypted: true,
            is_link: false,
        }];
        assert!(validate_listing(&encrypted, &ETH_LIMITS).is_err());

        let linked = vec![ArchiveEntry {
            path: "link.lnk".to_owned(),
            size: 1,
            is_directory: false,
            encrypted: false,
            is_link: true,
        }];
        assert!(validate_listing(&linked, &ETH_LIMITS).is_err());
    }

    #[test]
    fn detects_windows_reserved_device_names() {
        assert!(is_device_name("con"));
        assert!(is_device_name("CON.txt"));
        assert!(is_device_name("COM9"));
        assert!(is_device_name("LPT3.log"));
        assert!(is_device_name("NUL"));
        assert!(!is_device_name("console.txt"));
        assert!(!is_device_name("COMMAND"));
        assert!(!is_device_name("com0"));
    }
}
