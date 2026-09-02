use clap::{Parser, Subcommand};
use eli_lib::command::bootdisk::BootDiskSelectionSource;
use eli_lib::version_identifier::{EdgelessVersionIdentifier, ReleaseChannel, ReleaseStage};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "eli", version, about = "Edgeless Command Line Interface")]
struct Cli {
    /// Override the boot disk partition or mount path used by selecting commands.
    #[arg(short = 'b', long, global = true, value_name = "PARTITION")]
    bootdisk: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage Edgeless boot disks.
    Bootdisk {
        #[command(subcommand)]
        command: BootdiskCommand,
    },
}

#[derive(Debug, Subcommand)]
enum BootdiskCommand {
    /// List Edgeless boot disks connected to this computer.
    List,
    /// Get the selected Edgeless boot disk partition identifier.
    Get,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Bootdisk {
            command: BootdiskCommand::List,
        } => {
            let disks = eli_lib::command::bootdisk::list()?;
            let rows = disks
                .iter()
                .map(|disk| parse_version_identifier(&disk.version).map(|version| (disk, version)))
                .collect::<io::Result<Vec<_>>>()?;

            let bootdisk_width = rows
                .iter()
                .map(|(disk, _)| disk.mount_point.display().to_string().chars().count())
                .chain(std::iter::once("Bootdisk".len()))
                .max()
                .unwrap_or_default()
                + 5;
            let version_width = rows
                .iter()
                .map(|(_, identifier)| identifier.version.to_string().len())
                .chain(std::iter::once("Version".len()))
                .max()
                .unwrap_or_default()
                + 5;

            println!(
                "{:<bootdisk_width$}{:<version_width$}Release",
                "Bootdisk", "Version"
            );
            for (disk, identifier) in rows {
                println!(
                    "{:<bootdisk_width$}{:<version_width$}{}",
                    disk.mount_point.display(),
                    identifier.version.to_string(),
                    format_release(identifier)
                );
            }
        }
        Command::Bootdisk {
            command: BootdiskCommand::Get,
        } => {
            let selection = eli_lib::command::bootdisk::get(cli.bootdisk.as_deref())?;
            if selection.source == BootDiskSelectionSource::Automatic
                && selection.candidates.len() > 1
            {
                eprintln!(
                    "warning: found {} Edgeless boot disks; automatically selected {}; use --bootdisk <PARTITION> to override",
                    selection.candidates.len(),
                    selection.selected.partition.display()
                );
            }
            println!("{}", selection.selected.partition.display());
        }
    }

    Ok(())
}

fn format_release(identifier: EdgelessVersionIdentifier) -> String {
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
    fn parses_global_bootdisk_before_the_command() {
        let cli =
            Cli::try_parse_from(["eli", "--bootdisk", "/mnt/edgeless", "bootdisk", "get"]).unwrap();

        assert_eq!(cli.bootdisk, Some(PathBuf::from("/mnt/edgeless")));
        assert!(matches!(
            cli.command,
            Command::Bootdisk {
                command: BootdiskCommand::Get
            }
        ));
    }

    #[test]
    fn parses_short_global_bootdisk_after_the_command() {
        let cli =
            Cli::try_parse_from(["eli", "bootdisk", "get", "-b", "/Volumes/Edgeless"]).unwrap();

        assert_eq!(cli.bootdisk, Some(PathBuf::from("/Volumes/Edgeless")));
    }

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
