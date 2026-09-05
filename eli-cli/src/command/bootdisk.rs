use super::warn_automatic_bootdisk_selection;
use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::version_identifier::{EdgelessVersionIdentifier, ReleaseChannel, ReleaseStage};
use std::io;

const BOOTDISK_COLUMN_WIDTH: usize = 40;
const VERSION_COLUMN_WIDTH: usize = 12;

#[derive(Debug, Subcommand)]
pub(crate) enum BootdiskCommand {
    /// List Edgeless boot disks connected to this computer.
    List,
    /// Get the selected Edgeless boot disk partition identifier.
    Get,
}

pub(crate) fn execute(ctx: &Ctx, command: BootdiskCommand) -> io::Result<()> {
    match command {
        BootdiskCommand::List => list(),
        BootdiskCommand::Get => get(ctx),
    }
}

fn list() -> io::Result<()> {
    let disks = eli_lib::command::bootdisk::list()?;
    let rows = disks
        .iter()
        .map(|disk| parse_version_identifier(&disk.version).map(|version| (disk, version)))
        .collect::<io::Result<Vec<_>>>()?;

    println!(
        "{:<BOOTDISK_COLUMN_WIDTH$}{:<VERSION_COLUMN_WIDTH$}Release",
        "Bootdisk", "Version",
    );
    for (disk, identifier) in rows {
        println!(
            "{:<BOOTDISK_COLUMN_WIDTH$}{:<VERSION_COLUMN_WIDTH$}{}",
            disk.mount_point.display(),
            identifier.version.to_string(),
            format_release(identifier)
        );
    }
    Ok(())
}

fn get(ctx: &Ctx) -> io::Result<()> {
    let selection = ctx.bootdisk()?;
    warn_automatic_bootdisk_selection(selection);
    println!("{}", selection.selected.partition.display());
    Ok(())
}

pub(crate) fn format_release(identifier: EdgelessVersionIdentifier) -> String {
    let stage = match identifier.stage {
        ReleaseStage::Alpha => "Alpha",
        ReleaseStage::Beta => "Beta",
    };

    match identifier.channel {
        Some(ReleaseChannel::Official) => format!("{stage}(Official)"),
        None => stage.to_owned(),
    }
}

fn parse_version_identifier(version: &str) -> io::Result<EdgelessVersionIdentifier> {
    let version = version.trim_end_matches(['\r', '\n']);
    EdgelessVersionIdentifier::parse(version).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Edgeless version identifier {version:?}: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_boot_disk_versions() {
        let version = parse_version_identifier("Edgeless_Alpa_4.1.2\r\n").unwrap();

        assert_eq!(version.to_string(), "Edgeless_Alpha_4.1.2");
        assert_eq!(format_release(version), "Alpha");
    }

    #[test]
    fn formats_an_official_release() {
        let version = parse_version_identifier("Edgeless_Beta_Ofial_4.1.0_2").unwrap();

        assert_eq!(version.version.to_string(), "4.1.0");
        assert_eq!(format_release(version), "Beta(Official)");
    }

    #[test]
    fn rejects_invalid_boot_disk_versions() {
        let error = parse_version_identifier("unstructured-version").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unstructured-version"));
    }
}
