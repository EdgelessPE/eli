// `eli theme` 父命令模块。
//
// `apply` 热应用当前会话主题；`startup` 接管 PE 启动期默认主题与后置图标对账。
// 未来的 `theme store`（启动盘持久化）仍在独立设计中实现。

#[path = "apply.rs"]
mod apply;
#[path = "startup.rs"]
mod startup;

pub use apply::{
    ApplySummary, ComponentOutcome, ComponentStatus, EisStats, ExecutedRefresh, ThemeComponent,
    ThemeType, apply,
};
pub use startup::startup;
