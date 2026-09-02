//! Edgeless 版本标识符解析。

use std::error::Error;
use std::fmt;
use std::str::FromStr;

const PREFIX: &str = "Edgeless";

/// Edgeless 版本的开发阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStage {
    Alpha,
    Beta,
}

impl fmt::Display for ReleaseStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Alpha => "Alpha",
            Self::Beta => "Beta",
        })
    }
}

/// Edgeless 版本的分发渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChannel {
    /// 官方渠道，在 Edgeless 标识符中拼写为 `Ofial`。
    Official,
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Official => formatter.write_str("Ofial"),
        }
    }
}

/// 由三个部分组成的 Edgeless 版本号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgelessVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for EdgelessVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 将现有安装更新到此版本所需的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UpdateMethod {
    /// 仅更新必要的组件包。
    ComponentsOnly = 1,
    /// 更新必要的组件包和 `.wim` 文件。
    ComponentsAndWim = 2,
    /// 重新制作安装介质。
    RebuildRequired = 3,
}

impl fmt::Display for UpdateMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", *self as u8)
    }
}

/// 解析后的 Edgeless 版本标识符。
///
/// 当前格式同时包含 `channel` 和 `update_method`，兼容的旧格式则省略这两个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgelessVersionIdentifier {
    pub stage: ReleaseStage,
    pub channel: Option<ReleaseChannel>,
    pub version: EdgelessVersion,
    pub update_method: Option<UpdateMethod>,
}

impl EdgelessVersionIdentifier {
    /// 解析 Edgeless 版本标识符。
    pub fn parse(identifier: &str) -> Result<Self, ParseVersionIdentifierError> {
        identifier.parse()
    }
}

impl FromStr for EdgelessVersionIdentifier {
    type Err = ParseVersionIdentifierError;

    fn from_str(identifier: &str) -> Result<Self, Self::Err> {
        let fields: Vec<_> = identifier.split('_').collect();
        if fields.len() != 3 && fields.len() != 5 {
            return Err(ParseVersionIdentifierError::InvalidFormat);
        }
        if fields[0] != PREFIX {
            return Err(ParseVersionIdentifierError::InvalidPrefix(
                fields[0].to_owned(),
            ));
        }

        let stage = parse_stage(fields[1])?;
        if fields.len() == 3 {
            return Ok(Self {
                stage,
                channel: None,
                version: parse_version(fields[2])?,
                update_method: None,
            });
        }

        Ok(Self {
            stage,
            channel: Some(parse_channel(fields[2])?),
            version: parse_version(fields[3])?,
            update_method: Some(parse_update_method(fields[4])?),
        })
    }
}

impl fmt::Display for EdgelessVersionIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.channel, self.update_method) {
            (Some(channel), Some(update_method)) => write!(
                formatter,
                "{PREFIX}_{}_{}_{}_{}",
                self.stage, channel, self.version, update_method
            ),
            (None, None) => write!(formatter, "{PREFIX}_{}_{}", self.stage, self.version),
            _ => Err(fmt::Error),
        }
    }
}

/// Edgeless 版本标识符无法解析的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseVersionIdentifierError {
    InvalidFormat,
    InvalidPrefix(String),
    UnknownReleaseStage(String),
    UnknownChannel(String),
    InvalidVersion(String),
    UnknownUpdateMethod(String),
}

impl fmt::Display for ParseVersionIdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => formatter.write_str(
                "expected Edgeless_<Alpha|Beta>_<major.minor.patch> or Edgeless_<Alpha|Beta>_Ofial_<major.minor.patch>_<1|2|3>",
            ),
            Self::InvalidPrefix(prefix) => write!(formatter, "invalid Edgeless prefix: {prefix}"),
            Self::UnknownReleaseStage(stage) => {
                write!(formatter, "unknown Edgeless release stage: {stage}")
            }
            Self::UnknownChannel(channel) => {
                write!(formatter, "unknown Edgeless release channel: {channel}")
            }
            Self::InvalidVersion(version) => {
                write!(formatter, "invalid Edgeless version: {version}")
            }
            Self::UnknownUpdateMethod(method) => {
                write!(formatter, "unknown Edgeless update method: {method}")
            }
        }
    }
}

impl Error for ParseVersionIdentifierError {}

fn parse_stage(value: &str) -> Result<ReleaseStage, ParseVersionIdentifierError> {
    match value {
        "Alpha" | "Alpa" => Ok(ReleaseStage::Alpha),
        "Beta" => Ok(ReleaseStage::Beta),
        _ => Err(ParseVersionIdentifierError::UnknownReleaseStage(
            value.to_owned(),
        )),
    }
}

fn parse_channel(value: &str) -> Result<ReleaseChannel, ParseVersionIdentifierError> {
    match value {
        "Ofial" => Ok(ReleaseChannel::Official),
        _ => Err(ParseVersionIdentifierError::UnknownChannel(
            value.to_owned(),
        )),
    }
}

fn parse_version(value: &str) -> Result<EdgelessVersion, ParseVersionIdentifierError> {
    let invalid = || ParseVersionIdentifierError::InvalidVersion(value.to_owned());
    let mut components = value.split('.');
    let major = components
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let minor = components
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let patch = components
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    if components.next().is_some() {
        return Err(invalid());
    }

    Ok(EdgelessVersion {
        major,
        minor,
        patch,
    })
}

fn parse_update_method(value: &str) -> Result<UpdateMethod, ParseVersionIdentifierError> {
    match value {
        "1" => Ok(UpdateMethod::ComponentsOnly),
        "2" => Ok(UpdateMethod::ComponentsAndWim),
        "3" => Ok(UpdateMethod::RebuildRequired),
        _ => Err(ParseVersionIdentifierError::UnknownUpdateMethod(
            value.to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_identifier() {
        let identifier = EdgelessVersionIdentifier::parse("Edgeless_Beta_Ofial_4.1.0_2").unwrap();

        assert_eq!(identifier.stage, ReleaseStage::Beta);
        assert_eq!(identifier.channel, Some(ReleaseChannel::Official));
        assert_eq!(
            identifier.version,
            EdgelessVersion {
                major: 4,
                minor: 1,
                patch: 0
            }
        );
        assert_eq!(
            identifier.update_method,
            Some(UpdateMethod::ComponentsAndWim)
        );
    }

    #[test]
    fn parses_compatible_identifiers() {
        for (value, stage, version) in [
            (
                "Edgeless_Alpha_4.1.2",
                ReleaseStage::Alpha,
                EdgelessVersion {
                    major: 4,
                    minor: 1,
                    patch: 2,
                },
            ),
            (
                "Edgeless_Beta_4.1.0",
                ReleaseStage::Beta,
                EdgelessVersion {
                    major: 4,
                    minor: 1,
                    patch: 0,
                },
            ),
        ] {
            let identifier: EdgelessVersionIdentifier = value.parse().unwrap();
            assert_eq!(identifier.stage, stage);
            assert_eq!(identifier.version, version);
            assert_eq!(identifier.channel, None);
            assert_eq!(identifier.update_method, None);
        }
    }

    #[test]
    fn accepts_alpa_as_a_legacy_alpha_spelling() {
        let identifier: EdgelessVersionIdentifier = "Edgeless_Alpa_4.1.2".parse().unwrap();
        assert_eq!(identifier.stage, ReleaseStage::Alpha);
        assert_eq!(identifier.to_string(), "Edgeless_Alpha_4.1.2");
    }

    #[test]
    fn parses_every_update_method() {
        for (number, method) in [
            ("1", UpdateMethod::ComponentsOnly),
            ("2", UpdateMethod::ComponentsAndWim),
            ("3", UpdateMethod::RebuildRequired),
        ] {
            let value = format!("Edgeless_Beta_Ofial_4.1.0_{number}");
            assert_eq!(
                value
                    .parse::<EdgelessVersionIdentifier>()
                    .unwrap()
                    .update_method,
                Some(method)
            );
        }
    }

    #[test]
    fn rejects_malformed_identifiers() {
        for (value, expected) in [
            (
                "Other_Beta_4.1.0",
                ParseVersionIdentifierError::InvalidPrefix("Other".to_owned()),
            ),
            (
                "Edgeless_RC_4.1.0",
                ParseVersionIdentifierError::UnknownReleaseStage("RC".to_owned()),
            ),
            (
                "Edgeless_Beta_Community_4.1.0_2",
                ParseVersionIdentifierError::UnknownChannel("Community".to_owned()),
            ),
            (
                "Edgeless_Beta_4.1",
                ParseVersionIdentifierError::InvalidVersion("4.1".to_owned()),
            ),
            (
                "Edgeless_Beta_Ofial_4.1.0_4",
                ParseVersionIdentifierError::UnknownUpdateMethod("4".to_owned()),
            ),
            (
                "Edgeless_Beta_Ofial_4.1.0",
                ParseVersionIdentifierError::InvalidFormat,
            ),
        ] {
            assert_eq!(
                value.parse::<EdgelessVersionIdentifier>().unwrap_err(),
                expected
            );
        }
    }
}
