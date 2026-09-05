use super::warn_automatic_bootdisk_selection;
use clap::Subcommand;
use eli_lib::Ctx;
use std::io;

const VERSION_COLUMN_WIDTH: usize = 12;

#[derive(Debug, Subcommand)]
pub(crate) enum KernelCommand {
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
}
