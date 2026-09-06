use crate::http;
use crate::version_identifier::{EdgelessVersionIdentifier, ReleaseStage};
use serde::Deserialize;
use std::io;
use std::path::Path;

const ISO_INFO_URL: &str = "https://legacy.edgeless.top/api/v2/info/iso";
const ALPHA_DATA_URL: &str = "https://legacy.edgeless.top/api/v2/alpha/data";
const ALPHA_DOWNLOAD_URL: &str = "https://legacy.edgeless.top/api/v2/alpha/addr";

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

/// 查询最新 Alpha 内核的元数据。
pub fn latest_alpha_download_info(token: &str) -> io::Result<AlphaDownloadInfo> {
    validate_alpha_token(token)?;
    let response = http::get_text_with_query_parameter(ALPHA_DATA_URL, "token", token)?;
    parse_alpha_download_info(&response)
}

/// 查询最新 Alpha 内核版本。
pub fn latest_alpha_version(token: &str) -> io::Result<EdgelessVersionIdentifier> {
    validate_alpha_token(token)?;
    let response = http::get_text_with_query_parameter(ALPHA_DATA_URL, "token", token)?;
    parse_alpha_version_response(&response)
}

/// 将最新 Alpha 内核下载到指定路径，并在发布前校验临时文件。
pub fn download_latest_alpha<F>(
    token: &str,
    destination: &Path,
    overwrite: bool,
    validate: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    validate_alpha_token(token)?;
    http::download_with_progress_with_query_parameter_validated(
        ALPHA_DOWNLOAD_URL,
        "token",
        token,
        destination,
        overwrite,
        ("正在下载 Alpha WIM", "Alpha WIM 下载失败"),
        validate,
    )
}

/// Alpha 内核下载所需的服务端元数据。
#[derive(Debug, PartialEq, Eq)]
pub struct AlphaDownloadInfo {
    /// Alpha 内核版本。
    pub version: EdgelessVersionIdentifier,
    /// 下载后保存到启动盘根目录的 WIM 文件名。
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct AlphaDataResponse {
    #[serde(default)]
    iso_version: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    iso_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn validate_alpha_token(token: &str) -> io::Result<()> {
    // 与 Hub 的 JavaScript `String.length` 保持一致，按 UTF-16 码元计数。
    let length = token.encode_utf16().count();
    if !(4..=10).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Alpha token must contain between 4 and 10 characters",
        ));
    }
    Ok(())
}

fn parse_alpha_download_info(response: &str) -> io::Result<AlphaDownloadInfo> {
    let response = parse_alpha_data_response(response)?;
    let version = parse_alpha_version(alpha_version_field(&response)?)?;
    let name = alpha_name_field(&response)?.trim();
    validate_alpha_wim_name(name, version)?;

    Ok(AlphaDownloadInfo {
        version,
        name: name.to_owned(),
    })
}

fn alpha_version_field(response: &AlphaDataResponse) -> io::Result<&str> {
    response
        .iso_version
        .as_deref()
        .or(response.version.as_deref())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Edgeless Alpha data API did not return a version",
            )
        })
}

fn alpha_name_field(response: &AlphaDataResponse) -> io::Result<&str> {
    response
        .iso_name
        .as_deref()
        .or(response.name.as_deref())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Edgeless Alpha data API did not return a WIM file name",
            )
        })
}

fn parse_alpha_version_response(response: &str) -> io::Result<EdgelessVersionIdentifier> {
    let response = parse_alpha_data_response(response)?;
    parse_alpha_version(alpha_version_field(&response)?)
}

fn parse_alpha_data_response(response: &str) -> io::Result<AlphaDataResponse> {
    serde_json::from_str(response).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Edgeless Alpha data API response: {error}"),
        )
    })
}

fn parse_alpha_version(value: &str) -> io::Result<EdgelessVersionIdentifier> {
    let value = value.trim();
    if value == "0.0.0" {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no Edgeless Alpha kernel is currently available",
        ));
    }
    let identifier = EdgelessVersionIdentifier::parse(value)
        .or_else(|_| format!("Edgeless_Alpha_{value}").parse())
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Edgeless Alpha version from data API {value:?}: {error}"),
            )
        })?;
    if identifier.stage != ReleaseStage::Alpha {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned a non-Alpha version: {value:?}"),
        ));
    }
    Ok(identifier)
}

fn validate_alpha_wim_name(
    name: &str,
    expected_version: EdgelessVersionIdentifier,
) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned an invalid file name: {name:?}"),
        ));
    }
    let path = Path::new(name);
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wim"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned a non-WIM file name: {name:?}"),
        ));
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Edgeless Alpha data API returned an invalid file name: {name:?}"),
            )
        })?;
    if !stem.starts_with("Edgeless_Alpha_") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned a non-Alpha WIM name: {name:?}"),
        ));
    }
    let identified_name = EdgelessVersionIdentifier::parse(stem).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned an invalid WIM name {name:?}: {error}"),
        )
    })?;
    if identified_name.stage != ReleaseStage::Alpha
        || identified_name != expected_version
        || identified_name.to_string() != stem
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless Alpha data API returned inconsistent version and WIM name: {name:?}"),
        ));
    }
    Ok(())
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

    #[test]
    fn parses_alpha_data_from_the_hub_api_shape() {
        assert_eq!(
            parse_alpha_download_info(
                r#"{"iso_version":"4.1.2","iso_name":"Edgeless_Alpha_4.1.2.wim"}"#
            )
            .unwrap(),
            AlphaDownloadInfo {
                version: "Edgeless_Alpha_4.1.2".parse().unwrap(),
                name: "Edgeless_Alpha_4.1.2.wim".to_owned(),
            }
        );
    }

    #[test]
    fn accepts_alpha_data_legacy_field_names() {
        let info = parse_alpha_download_info(
            r#"{"version":"Edgeless_Alpha_4.1.2","name":"Edgeless_Alpha_4.1.2.wim"}"#,
        )
        .unwrap();

        assert_eq!(info.version.to_string(), "Edgeless_Alpha_4.1.2");
    }

    #[test]
    fn prefers_hub_fields_when_the_api_returns_both_field_sets() {
        let info = parse_alpha_download_info(
            r#"{"iso_version":"4.1.2","version":"9.9.9","iso_name":"Edgeless_Alpha_4.1.2.wim","name":"Edgeless_Alpha_9.9.9.wim"}"#,
        )
        .unwrap();

        assert_eq!(info.version.to_string(), "Edgeless_Alpha_4.1.2");
        assert_eq!(info.name, "Edgeless_Alpha_4.1.2.wim");
    }

    #[test]
    fn parses_alpha_version_without_requiring_a_download_name() {
        let version = parse_alpha_version_response(r#"{"version":"4.1.2"}"#).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Alpha_4.1.2");
    }

    #[test]
    fn rejects_unavailable_alpha_releases() {
        let error =
            parse_alpha_download_info(r#"{"iso_version":"0.0.0","iso_name":""}"#).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn rejects_alpha_file_names_that_do_not_match_the_reported_version() {
        let error = parse_alpha_download_info(
            r#"{"iso_version":"4.1.2","iso_name":"Edgeless_Alpha_4.1.3.wim"}"#,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_alpha_tokens_outside_the_hub_length_limits() {
        let short = validate_alpha_token("abc").unwrap_err();
        let long = validate_alpha_token("abcdefghijk").unwrap_err();
        let six_emojis = validate_alpha_token("😀😀😀😀😀😀").unwrap_err();

        assert_eq!(short.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(long.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(six_emojis.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_the_legacy_alpa_spelling_from_api_file_names() {
        let error = parse_alpha_download_info(
            r#"{"iso_version":"4.1.2","iso_name":"Edgeless_Alpa_4.1.2.wim"}"#,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_a_non_canonical_alpha_wim_version() {
        let error = parse_alpha_download_info(
            r#"{"iso_version":"4.1.2","iso_name":"Edgeless_Alpha_04.01.002.wim"}"#,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
