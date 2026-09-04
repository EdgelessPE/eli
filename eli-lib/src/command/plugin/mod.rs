mod attr;
mod delete;
mod list;
mod load;
pub mod localboost;
mod outdate;
mod package;
mod store;

pub use attr::set_attribute;
pub use delete::delete;
pub use list::list;
pub use load::{
    InputExpansionObserver, LoadOptions, LoadProgress, LoadProgressObserver, LoadResult,
    LoadStatus, LoadSummary, LocalBoostHandling, load,
};
pub use outdate::outdate;
pub use store::store;

use std::fmt;
use std::path::Path;

/// 插件包的加载属性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginAttribute {
    Normal,
    Frozen,
    LocalBoost,
}

impl PluginAttribute {
    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::Normal => "7z",
            Self::Frozen => "7zf",
            Self::LocalBoost => "7zl",
        }
    }

    pub(super) fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("7z") {
            Some(Self::Normal)
        } else if extension.eq_ignore_ascii_case("7zf") {
            Some(Self::Frozen)
        } else if extension.eq_ignore_ascii_case("7zl") {
            Some(Self::LocalBoost)
        } else {
            None
        }
    }
}

impl fmt::Display for PluginAttribute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(match self {
            Self::Normal => "Normal",
            Self::Frozen => "Frozen",
            Self::LocalBoost => "LocalBoost",
        })
    }
}

/// 从插件包文件名中解析出的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub author: String,
    pub attribute: PluginAttribute,
    pub automatically_built: bool,
}

#[cfg(test)]
fn test_resource_dir() -> std::path::PathBuf {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let path = std::env::temp_dir().join(format!(
        "eli-lib-plugin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_an_attribute_to_the_requested_display_width() {
        assert_eq!(
            format!("{:<16}", PluginAttribute::Normal),
            "Normal          "
        );
    }
}
