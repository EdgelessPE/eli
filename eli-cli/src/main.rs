use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "eli", version, about = "Edgeless Command Line Interface")]
struct Cli {
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
    }

    Ok(())
}
