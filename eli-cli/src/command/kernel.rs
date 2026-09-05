use super::warn_automatic_bootdisk_selection;
use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::command::kernel::DownloadResult;
use std::io;
use std::path::PathBuf;

const VERSION_COLUMN_WIDTH: usize = 12;

#[derive(Debug, Subcommand)]
pub(crate) enum KernelCommand {
    /// Download the latest Edgeless kernel ISO.
    Download {
        /// Directory in which to save the downloaded ISO.
        #[arg(short, long, value_name = "DIRECTORY")]
        directory: PathBuf,
        /// Replace an existing Edgeless.iso in the download directory.
        #[arg(short, long)]
        force: bool,
    },
    /// Show a kernel version from the current PE, the network, or a boot disk.
    Version {
        #[command(subcommand)]
        command: KernelVersionCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum KernelVersionCommand {
    /// Show the version of the currently running PE kernel.
    Current,
    /// Show the latest Beta kernel version available online.
    Latest,
    /// Show the version stored on the selected Edgeless boot disk.
    Bootdisk,
}

pub(crate) fn execute(ctx: &Ctx, command: KernelCommand) -> io::Result<()> {
    match command {
        KernelCommand::Download { directory, force } => {
            match eli_lib::command::kernel::download(&directory, force)? {
                DownloadResult::Downloaded(destination) => {
                    println!("Downloaded kernel ISO to {}", destination.display());
                }
                DownloadResult::Skipped(destination) => {
                    println!(
                        "Kernel ISO already exists at {}; skipped download",
                        destination.display()
                    );
                }
            }
            Ok(())
        }
        KernelCommand::Version { command } => match command {
            KernelVersionCommand::Current => print(eli_lib::command::kernel::current(ctx)?),
            KernelVersionCommand::Latest => print(eli_lib::command::kernel::latest()?),
            KernelVersionCommand::Bootdisk => {
                warn_automatic_bootdisk_selection(ctx.bootdisk()?);
                print(eli_lib::command::kernel::bootdisk(ctx)?)
            }
        },
    }
}

fn print(version: eli_lib::version_identifier::EdgelessVersionIdentifier) -> io::Result<()> {
    println!("{:<VERSION_COLUMN_WIDTH$}Release", "Version");
    println!(
        "{:<VERSION_COLUMN_WIDTH$}{}",
        version.version.to_string(),
        super::bootdisk::format_release(version),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use eli_lib::version_identifier::EdgelessVersionIdentifier;

    #[test]
    fn formats_an_identified_kernel_version_like_bootdisk_list() {
        let version = EdgelessVersionIdentifier::parse("Edgeless_Beta_Ofial_4.1.0_2").unwrap();

        assert_eq!(version.version.to_string(), "4.1.0");
        assert_eq!(
            super::super::bootdisk::format_release(version),
            "Beta(Official)"
        );
    }

    #[test]
    fn keeps_kernel_download_directory_required() {
        assert!(crate::Cli::try_parse_from(["eli", "kernel", "download"]).is_err());
    }
}
