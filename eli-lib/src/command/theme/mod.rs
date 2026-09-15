// `eli theme` 父命令模块。
//
// 首版只包含 `apply` 子命令：把主题包、资源包或壁纸应用到当前 Windows PE 会话。
// 未来的 `theme store`（启动盘持久化）等子命令在各自设计中独立立项，这里不提前
// 混入其职责。

#[path = "apply.rs"]
mod apply;

pub use apply::{
    ApplySummary, ComponentOutcome, ComponentStatus, EisStats, ExecutedRefresh, ThemeComponent,
    ThemeType, apply,
};
