use clap::{Parser, Subcommand};
use eli_lib::command::bootdisk::BootDiskSelectionSource;
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
            for disk in eli_lib::command::bootdisk::list()? {
                let version = disk.version.trim_end_matches(['\r', '\n']);
                println!("{}\t{version}", disk.mount_point.display());
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
}
