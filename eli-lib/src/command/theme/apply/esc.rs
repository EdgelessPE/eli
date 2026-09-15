// `.esc` 开始菜单配置。
//
// ESC 是 PECMD/WCS 兼容脚本，继续交由经过依赖管理模块验证的 PECMD 解释，
// 不在 Eli 内实现 PECMD/WCS 子集解析器，也不通过简单关键字 allowlist/denylist
// 冒充安全沙箱。执行前只做不会改变合法 PECMD 脚本语义的传输级/损坏文件预检。

use std::io;
use std::path::{Path, PathBuf};

use super::ThemeBackend;
use super::refresh::{RefreshPlan, RefreshRequest};

/// ESC 脚本的合理最大文件大小。
pub const ESC_MAX_SIZE: u64 = 8 << 20;

/// ESC 预检结果。
#[derive(Debug, Clone)]
pub struct PreparedEsc {
    /// 稳定绝对脚本路径。
    pub script: PathBuf,
}

/// 传输级预检：普通非空文件、大小上限、BOM 编码无明显截断。
pub fn precheck_esc_script(script: &Path) -> io::Result<()> {
    let metadata = std::fs::metadata(script)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ESC input is not a regular file: {}", script.display()),
        ));
    }
    let size = metadata.len();
    if size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ESC input is empty: {}", script.display()),
        ));
    }
    if size > ESC_MAX_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "ESC input exceeds the size limit ({} bytes > {ESC_MAX_SIZE}): {}",
                size,
                script.display()
            ),
        ));
    }
    let contents = std::fs::read(script)?;
    check_encoding_integrity(&contents).map_err(|message| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "ESC input {} has a truncated multibyte encoding: {message}",
                script.display()
            ),
        )
    })
}

/// 检查 UTF-16/UTF-8 BOM 编码没有明显截断；ANSI/无 BOM 脚本直接接受。
/// 不会重编码历史脚本，也不强制要求 UTF-8。
fn check_encoding_integrity(contents: &[u8]) -> Result<(), String> {
    let Some(prefix) = contents.get(..2) else {
        return Ok(());
    };
    match prefix {
        [0xFF, 0xFE] | [0xFE, 0xFF] => {
            let body = contents.len() - 2;
            if !body.is_multiple_of(2) || body == 0 {
                return Err("UTF-16 BOM 后内容长度不是偶数或为空".to_owned());
            }
            let little_endian = prefix == [0xFF, 0xFE];
            let units = contents[2..].chunks_exact(2).map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            });
            if std::char::decode_utf16(units).any(|character| character.is_err()) {
                return Err("UTF-16 正文包含不完整的代理项".to_owned());
            }
            Ok(())
        }
        [0xEF, 0xBB] if contents.get(2) == Some(&0xBF) => {
            let remainder = &contents[3..];
            std::str::from_utf8(remainder)
                .map(|_| ())
                .map_err(|error| format!("UTF-8 BOM 后正文无效或截断：{error}"))
        }
        _ => Ok(()),
    }
}

/// 准备 ESC：稳定绝对路径 + 传输级预检。
pub fn prepare_esc(script: &Path) -> io::Result<PreparedEsc> {
    let script = std::fs::canonicalize(script)?;
    precheck_esc_script(&script)?;
    Ok(PreparedEsc { script })
}

/// 提交 ESC：通过 PECMD LOAD 同步执行；非零退出/超时/无法启动均为组件失败，
/// 且不承诺通用回滚。
pub fn commit_esc(
    prepared: &PreparedEsc,
    backend: &dyn ThemeBackend,
    refresh: &mut RefreshPlan,
) -> io::Result<()> {
    backend.execute_esc(&prepared.script).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "StartIsBackConfig.esc failed to execute and cannot be rolled back: {} ({error})",
                prepared.script.display()
            ),
        )
    })?;
    refresh.request(RefreshRequest::ExplorerRestart);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eli-theme-esc-{}-{}",
            std::process::id(),
            super::super::transaction::unique_transaction_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::File::create(&path).unwrap();
        if !contents.is_empty() {
            std::fs::write(&path, contents).unwrap();
        }
        path
    }

    #[test]
    fn accepts_utf16_and_plain_scripts() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain([b'R', 0, b'E', 0, b'G', 0, b'I', 0])
            .collect();
        assert!(check_encoding_integrity(&utf16).is_ok());
        assert!(check_encoding_integrity(b"REGI HKLM\\x").is_ok());
        assert!(check_encoding_integrity(b"").is_ok());
    }

    #[test]
    fn rejects_truncated_utf16_and_empty_scripts() {
        // UTF-16LE BOM + 3 个字节：正文不是偶数长度，视为明显截断。
        let truncated: Vec<u8> = [0xFF, 0xFE].into_iter().chain([0u8, 1, 2]).collect();
        assert!(check_encoding_integrity(&truncated).is_err());
        // 只有 BOM 没有正文。
        assert!(check_encoding_integrity(&[0xFE, 0xFF]).is_err());
        let unpaired_surrogate = [0xFF, 0xFE, 0x00, 0xD8];
        assert!(check_encoding_integrity(&unpaired_surrogate).is_err());
    }

    #[test]
    fn rejects_truncated_utf8_after_a_bom() {
        assert!(check_encoding_integrity(&[0xEF, 0xBB, 0xBF, 0xE2, 0x82]).is_err());
        assert!(check_encoding_integrity(&[0xEF, 0xBB, 0xBF, b'E', b'X', b'I', b'T']).is_ok());
    }

    #[test]
    fn precheck_rejects_empty_and_oversized_files() {
        let empty = test_file("empty.esc", &[]);
        assert_eq!(
            precheck_esc_script(&empty).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(empty.parent().unwrap()).unwrap();

        let oversized = test_file("big.esc", &[0u8; (ESC_MAX_SIZE as usize) + 1]);
        assert_eq!(
            precheck_esc_script(&oversized).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(oversized.parent().unwrap()).unwrap();
    }
}
