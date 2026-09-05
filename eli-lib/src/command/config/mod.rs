mod list;
mod set;

pub use list::{ConfigEntry, list};
pub use set::{SetOptions, set, set_with_options};

use crate::version_identifier::EdgelessVersion;
use std::fmt;
use std::io;

/// 可由目录存在状态表示的启动盘配置项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BooleanConfig {
    pub key: &'static str,
    higher_than: EdgelessVersion,
    lower_than: Option<EdgelessVersion>,
}

impl BooleanConfig {
    pub fn is_available_for(self, version: EdgelessVersion) -> bool {
        version > self.higher_than
            && self
                .lower_than
                .is_none_or(|lower_than| version < lower_than)
    }

    pub fn supported_version_range(self) -> String {
        match self.lower_than {
            Some(lower_than) => format!("> {} and < {}", self.higher_than, lower_than),
            None => format!("> {}", self.higher_than),
        }
    }
}

const fn version(major: u32, minor: u32, patch: u32) -> EdgelessVersion {
    EdgelessVersion {
        major,
        minor,
        patch,
    }
}

pub(super) const BOOLEAN_CONFIGS: [BooleanConfig; 14] = [
    BooleanConfig {
        key: "DisablePinBrowsers",
        higher_than: version(4, 0, 2),
        lower_than: None,
    },
    BooleanConfig {
        key: "RebootDefault",
        higher_than: version(3, 1, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "Developer",
        higher_than: version(2, 2, 0),
        lower_than: Some(version(4, 0, 0)),
    },
    BooleanConfig {
        key: "NoOutDateCheck",
        higher_than: version(2, 2, 0),
        lower_than: Some(version(4, 0, 0)),
    },
    BooleanConfig {
        key: "DisableUSBManager",
        higher_than: version(3, 0, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "DisableSmartISO",
        higher_than: version(3, 0, 6),
        lower_than: None,
    },
    BooleanConfig {
        key: "UnfoldRibbon",
        higher_than: version(3, 0, 5),
        lower_than: Some(version(4, 0, 0)),
    },
    BooleanConfig {
        key: "DisableRecycleBin",
        higher_than: version(3, 0, 6),
        lower_than: None,
    },
    BooleanConfig {
        key: "AutoUnattend",
        higher_than: version(3, 0, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "OrderDrvAnotherWay",
        higher_than: version(3, 0, 5),
        lower_than: None,
    },
    BooleanConfig {
        key: "DisableLoadScreen",
        higher_than: version(3, 1, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "UpActDrv",
        higher_than: version(3, 0, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "WinFirst",
        higher_than: version(3, 0, 0),
        lower_than: None,
    },
    BooleanConfig {
        key: "MountEveryPartition",
        higher_than: version(3, 1, 5),
        lower_than: None,
    },
];

pub(super) const HOMEPAGE_HIGHER_THAN: EdgelessVersion = version(4, 0, 2);

pub(super) fn boolean_config(key: &str) -> Option<BooleanConfig> {
    BOOLEAN_CONFIGS
        .iter()
        .copied()
        .find(|config| config.key.eq_ignore_ascii_case(key))
}

pub(super) fn parse_bootdisk_version(version: &str) -> io::Result<EdgelessVersion> {
    let value = version.trim_end_matches(['\r', '\n']);
    value
        .parse::<crate::version_identifier::EdgelessVersionIdentifier>()
        .map(|identifier| identifier.version)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Edgeless version identifier {value:?}: {error}"),
            )
        })
}

pub(super) fn invalid_key(key: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unknown config key {key:?}; run `eli config list` to see supported keys"),
    )
}

pub(super) fn unavailable(
    key: &str,
    version: EdgelessVersion,
    range: impl fmt::Display,
) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "config key {key} is unavailable on Edgeless {version}; supported versions are {range}"
        ),
    )
}
