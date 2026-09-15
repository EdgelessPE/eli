// 可复用的 Shell Link（`.lnk`）读写能力。
//
// 供 `theme apply` 的 EIS 组件使用：只修改图标位置，不改目标、参数、工作目录、
// 描述或热键。Windows 实现位于 `desktop_icon/windows.rs`（COM `IShellLinkW` +
// `IPersistFile`）；其他平台保留统一接口并在调用时返回不支持错误。

use std::io;
use std::path::Path;

/// 修改 `.lnk` 的图标位置（绝对 `.ico` 路径，图标索引 0）。
pub fn set_icon_location(link: &Path, icon: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::set_icon_location(link, icon)
    }
    #[cfg(not(windows))]
    {
        let _ = (link, icon);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Shell Link modification is only implemented on Windows",
        ))
    }
}

#[cfg(windows)]
#[path = "desktop_icon/windows.rs"]
mod windows;
