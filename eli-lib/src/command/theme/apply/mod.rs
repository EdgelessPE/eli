// `theme apply` 内部共享类型与系统后端抽象。
//
// 本模块只承载跨组件共享的结构与接口：组件各自的具体预检/提交逻辑位于
// `archive`、`transaction`、`refresh`、`wallpaper`、`eis`、`ems`、`esc`、
// `ess` 文件中，真实 Win32 副作用集中在 `windows`。非 Windows 生产构建只保留
// 公开结果类型和平台拒绝入口；测试构建仍编译完整编排逻辑并使用 fake 后端验证。

#[cfg(any(windows, test))]
pub(super) mod archive;
#[cfg(any(windows, test))]
pub(in crate::command::theme) mod eis;
#[cfg(any(windows, test))]
pub(super) mod ems;
#[cfg(any(windows, test))]
pub(super) mod esc;
#[cfg(any(windows, test))]
pub(super) mod ess;
#[cfg(any(windows, test))]
pub(super) mod event_log;
#[cfg(any(windows, test))]
pub(in crate::command::theme) mod refresh;
#[cfg(test)]
pub mod test_support;
#[cfg(any(windows, test))]
pub(super) mod transaction;
#[cfg(any(windows, test))]
pub(super) mod wallpaper;
#[cfg(windows)]
pub(in crate::command::theme) mod windows;

#[cfg(any(windows, test))]
use std::io;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

/// 外层主题包类型（按扩展名路由）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeType {
    Eth,
    Eis,
    Ems,
    Esc,
    Ess,
    Els,
    Jpg,
}

impl ThemeType {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Eth => "eth",
            Self::Eis => "eis",
            Self::Ems => "ems",
            Self::Esc => "esc",
            Self::Ess => "ess",
            Self::Els => "els",
            Self::Jpg => "jpg",
        }
    }
}

/// 主题组件（.eth 根目录的规范组件名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeComponent {
    Wallpaper,
    LoadScreen,
    IconPack,
    MouseStyle,
    StartIsBackConfig,
    SystemIconPack,
}

impl ThemeComponent {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Wallpaper => "WallPaper.jpg",
            Self::LoadScreen => "LoadScreen.els",
            Self::IconPack => "IconPack.eis",
            Self::MouseStyle => "MouseStyle.ems",
            Self::StartIsBackConfig => "StartIsBackConfig.esc",
            Self::SystemIconPack => "SystemIconPack.ess",
        }
    }
}

/// 单个组件的应用结果状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentStatus {
    Applied,
    AppliedWithWarnings(Vec<String>),
    Skipped,
    Failed(String),
}

/// 单个组件的结果记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentOutcome {
    pub component: ThemeComponent,
    pub status: ComponentStatus,
    /// 失败来自 Win32/系统 I/O 时保留原始错误码；逻辑校验失败为 None。
    pub windows_error_code: Option<i32>,
}

/// EIS 快捷方式修改统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EisStats {
    /// 已检查且找到对应 `.lnk` 的数量。
    pub checked: usize,
    /// 成功修改的 `.lnk` 数量。
    pub updated: usize,
    /// 图标位置已经正确、无需写回的 `.lnk` 数量。
    pub unchanged: usize,
    /// 未找到对应 `.lnk` 的图标数量。
    pub not_found: usize,
    /// 修改失败的 `.lnk` 数量。
    pub failed: usize,
}

/// 最终实际执行的 Shell/资源刷新动作。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutedRefresh {
    pub explorer_restarted: bool,
    pub shortcut_notified: usize,
    pub cursors_refreshed: bool,
    pub icon_cache_invalidated: bool,
    /// 刷新阶段的非致命问题（例如缓存清理失败）。
    pub warnings: Vec<String>,
}

/// `theme apply` 的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplySummary {
    /// 用户传入的外层主题源路径。
    pub source: PathBuf,
    pub outer: ThemeType,
    pub components: Vec<ComponentOutcome>,
    /// 迁移与兼容性警告（例如 ELS 跳过、未知根条目）。
    pub warnings: Vec<String>,
    pub eis: EisStats,
    pub refresh: ExecutedRefresh,
}

impl ApplySummary {
    pub fn applied(&self) -> usize {
        self.components
            .iter()
            .filter(|outcome| matches!(outcome.status, ComponentStatus::Applied))
            .count()
    }

    pub fn applied_with_warnings(&self) -> usize {
        self.components
            .iter()
            .filter(|outcome| matches!(outcome.status, ComponentStatus::AppliedWithWarnings(_)))
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.components
            .iter()
            .filter(|outcome| matches!(outcome.status, ComponentStatus::Skipped))
            .count()
    }

    pub fn failed(&self) -> usize {
        self.components
            .iter()
            .filter(|outcome| matches!(outcome.status, ComponentStatus::Failed(_)))
            .count()
    }

    /// 全部成功（失败数为 0；Skipped 不算失败）。
    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

/// 当前 PE 会话的主题运行时路径，集中解析避免各组件重复猜测。
#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
pub struct ThemePaths {
    /// %SystemRoot%。
    pub system_root: PathBuf,
    /// <系统卷>\Users\Theme\eli\staging
    pub staging_root: PathBuf,
    /// <系统卷>\\Users\\Theme\\eli\\wallpaper
    pub wallpaper_dir: PathBuf,
    /// <系统卷>\\Users\\Icon（Edgeless 兼容图标根目录）
    pub icon_root: PathBuf,
    /// %SystemRoot%\\Cursors\\Edgeless
    pub cursor_root: PathBuf,
    /// 桌面根目录（当前用户桌面、公共桌面、PE 兼容目录；去重）。
    pub desktop_roots: Vec<PathBuf>,
    /// Shell 用户图标缓存目录（AppData\\Local\\Microsoft\\Windows\\Explorer）
    pub icon_cache_dir: PathBuf,
}

#[cfg(any(windows, test))]
impl ThemePaths {
    /// 会话稳定壁纸文件路径。
    pub fn wallpaper_file(&self) -> PathBuf {
        self.wallpaper_dir.join("current.jpg")
    }
}

/// 光标注册表快照（撤销 EMS 写入用）。
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValueSnapshot {
    pub value_type: u32,
    pub data: Vec<u8>,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Default)]
pub struct CursorSnapshot {
    /// 17 个槽位的当前值；None 表示 HKCU 中没有该槽位。
    pub slots: [Option<RegistryValueSnapshot>; 17],
    /// HKCU\Control Panel\Cursors 的默认值（当前方案名）。
    pub default_scheme: Option<RegistryValueSnapshot>,
    /// HKCU\Control Panel\Cursors\Schemes\<目标方案> 的现值；None 表示原本不存在。
    pub target_scheme: Option<RegistryValueSnapshot>,
    /// 本次写入的方案名。
    pub scheme_name: String,
}

/// 系统后端抽象。
///
/// Windows 真实实现位于 `windows.rs`（7-Zip/PECMD 进程、注册表、COM、
/// SPI、Explorer 生命周期、ACL 与命名互斥体）；单元测试使用 fake 实现，
/// 使编排逻辑、组件预检与提交时序能够在 Windows、Linux、macOS 上验证。
#[cfg(any(windows, test))]
pub trait ThemeBackend: Send + Sync {
    /// 当前会话的主题运行时路径。
    fn theme_paths(&self) -> io::Result<ThemePaths>;
    /// 探测 7-Zip 可执行文件可用（在创建 staging 前完成）。
    fn require_seven_zip(&self) -> io::Result<()>;
    /// 探测 PECMD 可执行文件可用（在创建 staging 前的副作用外完成）。
    fn require_pecmd(&self) -> io::Result<()>;
    /// 校验当前进程与已存在的 Explorer 属于同一用户和会话。
    fn verify_shell_context(&self) -> io::Result<()>;
    /// 列出归档条目（7z l -slt -sccUTF-8）。
    fn list_archive(&self, source: &Path) -> io::Result<Vec<archive::ArchiveEntry>>;
    /// 把白名单条目解压到唯一 staging 目录。
    fn extract_archive_entries(
        &self,
        source: &Path,
        destination: &Path,
        entries: &[String],
    ) -> io::Result<()>;
    /// 预检：内容必须是可完整解码的静态 JPEG。
    fn decode_jpeg(&self, bytes: &[u8]) -> io::Result<()>;
    /// 预检：内容必须是可完整解码的 ICO。
    fn decode_icon(&self, bytes: &[u8]) -> io::Result<()>;
    /// 预检：内容必须是可完整解码的普通静态图片（非 ICO 专用校验）。
    fn decode_plain_image(&self, bytes: &[u8]) -> io::Result<()>;
    /// 预检：光标文件必须能被 Windows 加载（LoadCursorFromFileW）。
    fn validate_cursor_file(&self, path: &Path) -> io::Result<()>;
    /// 通过 PECMD 把会话稳定壁纸应用到当前会话（WALL）。
    fn apply_wallpaper(&self, image: &Path) -> io::Result<()>;
    /// 通过 PECMD 同步执行 ESC 脚本（LOAD）。
    fn execute_esc(&self, script: &Path) -> io::Result<()>;
    /// 快照本次会改动的光标注册表状态。
    fn snapshot_cursors(&self, target_scheme: &str) -> io::Result<CursorSnapshot>;
    /// 写入当前光标值；数组中 None 表示该槽位不修改。
    fn write_cursor_slots(&self, values: &[Option<String>; 17]) -> io::Result<()>;
    /// 写入 Schemes\\<方案名>（REG_SZ 完整 17 槽逗号分隔串；None 槽为空字段）。
    fn write_cursor_scheme(&self, name: &str, values: &[Option<String>; 17]) -> io::Result<()>;
    /// 把 HKCU\\Control Panel\\Cursors 默认值写为方案名。
    fn write_cursor_default_scheme(&self, name: &str) -> io::Result<()>;
    /// 恢复光标注册表快照。
    fn restore_cursors(&self, snapshot: &CursorSnapshot) -> io::Result<()>;
    /// 把光标目录发布到最终位置（原子发布），返回最终绝对路径。
    fn publish_cursor_directory(&self, source: &Path, id: &str) -> io::Result<PathBuf>;
    /// 移除本次发布的光标目录（回滚用）。
    fn remove_cursor_directory(&self, directory: &Path) -> io::Result<()>;
    /// 通知系统重载光标（SPI_SETCURSORS）。
    fn refresh_cursors(&self) -> io::Result<()>;
    /// 在专用单线程 COM STA 中批量修改 .lnk 图标；逐项结果与输入顺序一致。
    fn modify_shortcut_icons(
        &self,
        changes: &[(PathBuf, PathBuf)],
    ) -> io::Result<Vec<io::Result<bool>>>;
    /// 对成功修改的 .lnk 发送定点 Shell 通知。
    fn notify_shortcuts(&self, links: &[PathBuf]) -> io::Result<()>;
    /// 当前会话是否正在运行 Explorer。
    fn shell_is_running(&self) -> io::Result<bool>;
    /// 停止当前会话的 Explorer（未运行时视为成功）。
    fn stop_shell(&self) -> io::Result<()>;
    /// 启动当前会话的 Explorer（仅由编排器在需要时调用）。
    fn start_shell(&self) -> io::Result<()>;
    /// 以临时最低必要写权限执行操作，结束后恢复原安全描述符。
    fn with_temporary_write_permission(
        &self,
        file: &Path,
        operation: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()>;
}
