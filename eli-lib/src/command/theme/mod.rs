// `eli theme` 父命令模块。
//
// `apply` 热应用当前会话主题；`startup` 接管 PE 启动期默认主题与后置图标对账；
// `store` 按旧版主题安装契约持久化到启动盘。

#[path = "apply.rs"]
mod apply;
#[path = "delete.rs"]
mod delete;
#[path = "list.rs"]
mod list;
#[path = "startup.rs"]
mod startup;
#[path = "storage.rs"]
mod storage;
#[path = "store.rs"]
mod store;

pub use apply::{
    ApplySummary, ComponentOutcome, ComponentStatus, EisStats, ExecutedRefresh, ThemeComponent,
    ThemeType, apply,
};
pub use delete::{ThemeDeleteSummary, ThemeDeleteTarget, delete};
pub use list::{ListedThemeResource, ThemeListSummary, list};
pub use startup::startup;
pub use store::{StoreSummary, store};
