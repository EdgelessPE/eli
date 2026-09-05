use crate::http;
use crate::version_identifier::{EdgelessVersionIdentifier, ReleaseStage};
use serde::Deserialize;
use std::io;

const ISO_INFO_URL: &str = "https://legacy.edgeless.top/api/v2/info/iso";

/// 查询 Edgeless 最新 Beta 内核版本。
pub fn latest_beta_version() -> io::Result<EdgelessVersionIdentifier> {
    let response = http::get_text(ISO_INFO_URL)?;
    parse_latest_beta_version(&response)
}

/// 查询 Edgeless ISO 的文件名和下载地址。
pub fn latest_iso_download_info() -> io::Result<IsoDownloadInfo> {
    let response = http::get_text(ISO_INFO_URL)?;
    parse_iso_download_info(&response)
}

/// Edgeless ISO 下载所需的服务端元数据。
#[derive(Debug, PartialEq, Eq)]
pub struct IsoDownloadInfo {
    /// ISO 文件名。
    pub name: String,
    /// ISO 下载地址。
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct IsoInfoResponse {
    version: String,
}

fn parse_latest_beta_version(response: &str) -> io::Result<EdgelessVersionIdentifier> {
    let response: IsoInfoResponse = serde_json::from_str(response).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Edgeless ISO info API response: {error}"),
        )
    })?;

    let value = response.version.trim();
    let identifier = match EdgelessVersionIdentifier::parse(value) {
        Ok(identifier) => identifier,
        Err(_) => format!("Edgeless_Beta_{value}")
            .parse()
            .map_err(|error| invalid_version(value, error))?,
    };
    if identifier.stage != ReleaseStage::Beta {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless ISO info API returned a non-Beta version: {value:?}"),
        ));
    }
    Ok(identifier)
}

fn invalid_version(value: &str, error: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid Edgeless Beta version from ISO info API {value:?}: {error}"),
    )
}

#[derive(Debug, Deserialize)]
struct IsoDownloadInfoResponse {
    #[serde(alias = "Name")]
    name: String,
    #[serde(alias = "URL")]
    url: String,
}

fn parse_iso_download_info(response: &str) -> io::Result<IsoDownloadInfo> {
    let response: IsoDownloadInfoResponse = serde_json::from_str(response).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Edgeless ISO info API response: {error}"),
        )
    })?;
    let url = response.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless ISO info API returned an invalid URL: {url:?}"),
        ));
    }
    Ok(IsoDownloadInfo {
        name: response.name.trim().to_owned(),
        url: url.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_beta_version_from_the_api() {
        let version = parse_latest_beta_version(r#"{"version":"4.1.0"}"#).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Beta_4.1.0");
    }

    #[test]
    fn preserves_a_complete_beta_identifier_from_the_api() {
        let version =
            parse_latest_beta_version(r#"{"version":"Edgeless_Beta_Ofial_4.1.0_2"}"#).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Beta_Ofial_4.1.0_2");
    }

    #[test]
    fn rejects_a_non_beta_version_from_the_api() {
        let error = parse_latest_beta_version(r#"{"version":"Edgeless_Alpha_4.1.2"}"#).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("non-Beta"));
    }

    #[test]
    fn parses_iso_download_name_and_url() {
        assert_eq!(
            parse_iso_download_info(
                r#"{"name":"Edgeless_Beta.iso","url":"https://example.com/Edgeless_Beta.iso"}"#
            )
            .unwrap(),
            IsoDownloadInfo {
                name: "Edgeless_Beta.iso".to_owned(),
                url: "https://example.com/Edgeless_Beta.iso".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_an_invalid_iso_download_url() {
        let error =
            parse_iso_download_info(r#"{"name":"Edgeless.iso","url":"file:///iso"}"#).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
