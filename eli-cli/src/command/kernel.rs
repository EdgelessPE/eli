use super::warn_automatic_bootdisk_selection;
use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::command::kernel::{DownloadResult, StoreResult};
use std::io;
use std::path::PathBuf;

const VERSION_COLUMN_WIDTH: usize = 12;

#[derive(Debug, Subcommand)]
pub(crate) enum KernelCommand {
    /// Manage Edgeless Alpha kernels.
    Alpha {
        /// Invitation token required for Alpha online operations.
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
        #[command(subcommand)]
        command: KernelAlphaCommand,
    },
    /// Download the latest Edgeless kernel ISO.
    Download {
        /// Directory in which to save the downloaded ISO.
        #[arg(short, long, value_name = "DIRECTORY")]
        directory: PathBuf,
        /// Replace an existing Edgeless.iso in the download directory.
        #[arg(short, long)]
        force: bool,
    },
    /// Store an Edgeless ISO or WIM on the selected boot disk.
    Store {
        /// Path to the Edgeless ISO or WIM file.
        path: PathBuf,
        /// Original Edgeless release file name, used when the local file was renamed.
        #[arg(long, value_name = "FILE_NAME")]
        name: Option<String>,
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

#[derive(Debug, Subcommand)]
pub(crate) enum KernelAlphaCommand {
    /// Download the latest Alpha kernel WIM.
    Download {
        /// Directory in which to save the downloaded WIM.
        #[arg(short, long, value_name = "DIRECTORY")]
        directory: PathBuf,
        /// Replace an existing Alpha WIM in the download directory.
        #[arg(short, long)]
        force: bool,
    },
    /// Show an Alpha kernel version from the network or a boot disk.
    Version {
        #[command(subcommand)]
        command: KernelAlphaVersionCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum KernelAlphaVersionCommand {
    /// Show the latest Alpha kernel version available online.
    Latest,
    /// Show the latest Alpha kernel version stored on the selected boot disk.
    Bootdisk,
}

pub(crate) fn execute(ctx: &Ctx, command: KernelCommand) -> io::Result<()> {
    match command {
        KernelCommand::Alpha { token, command } => execute_alpha(ctx, token.as_deref(), command),
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
        KernelCommand::Store { path, name } => {
            let result = eli_lib::command::kernel::store(ctx, &path, name.as_deref())?;
            print_store_result(result);
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

fn execute_alpha(ctx: &Ctx, token: Option<&str>, command: KernelAlphaCommand) -> io::Result<()> {
    match command {
        KernelAlphaCommand::Download { directory, force } => {
            let token = require_alpha_token(token)?;
            match eli_lib::command::kernel::alpha::download(&directory, force, token)? {
                eli_lib::command::kernel::alpha::DownloadResult::Downloaded(destination) => {
                    println!("Downloaded Alpha kernel WIM to {}", destination.display());
                }
                eli_lib::command::kernel::alpha::DownloadResult::Skipped(destination) => {
                    println!(
                        "Alpha kernel WIM already exists at {}; skipped download",
                        destination.display()
                    );
                }
            }
            Ok(())
        }
        KernelAlphaCommand::Version { command } => match command {
            KernelAlphaVersionCommand::Latest => print(eli_lib::command::kernel::alpha::latest(
                require_alpha_token(token)?,
            )?),
            KernelAlphaVersionCommand::Bootdisk => {
                warn_automatic_bootdisk_selection(ctx.bootdisk()?);
                print(eli_lib::command::kernel::alpha::bootdisk(ctx)?)
            }
        },
    }
}

fn require_alpha_token(token: Option<&str>) -> io::Result<&str> {
    token
        // 与 Hub 的 JavaScript `String.length` 保持一致，按 UTF-16 码元计数。
        .filter(|token| (4..=10).contains(&token.encode_utf16().count()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--token <TOKEN> with 4 to 10 characters is required for Alpha online operations",
            )
        })
}

fn print_store_result(result: StoreResult) {
    if result.updated_edgeless {
        println!(
            "Stored Edgeless {} and kernel WIM at {}",
            result.version,
            result.wim_path.display()
        );
    } else {
        println!("Stored kernel WIM at {}", result.wim_path.display());
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
    use super::*;
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

    #[test]
    fn parses_alpha_latest_with_a_token() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "kernel",
            "alpha",
            "--token",
            "invite-token",
            "version",
            "latest",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Kernel {
                command: KernelCommand::Alpha {
                    token: Some(token),
                    command: KernelAlphaCommand::Version {
                        command: KernelAlphaVersionCommand::Latest,
                    },
                },
            } if token == "invite-token"
        ));
    }

    #[test]
    fn parses_alpha_bootdisk_version_without_a_token() {
        let cli =
            crate::Cli::try_parse_from(["eli", "kernel", "alpha", "version", "bootdisk"]).unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Kernel {
                command: KernelCommand::Alpha {
                    token: None,
                    command: KernelAlphaCommand::Version {
                        command: KernelAlphaVersionCommand::Bootdisk,
                    },
                },
            }
        ));
    }

    #[test]
    fn parses_alpha_download_with_a_token() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "kernel",
            "alpha",
            "--token",
            "invite-token",
            "download",
            "--directory",
            "/tmp/alpha",
            "--force",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Kernel {
                command: KernelCommand::Alpha {
                    command: KernelAlphaCommand::Download {
                        directory,
                        force: true,
                    },
                    ..
                },
            } if directory == std::path::Path::new("/tmp/alpha")
        ));
    }

    #[test]
    fn keeps_alpha_download_directory_required() {
        assert!(
            crate::Cli::try_parse_from([
                "eli",
                "kernel",
                "alpha",
                "--token",
                "invite-token",
                "download",
            ])
            .is_err()
        );
    }

    #[test]
    fn requires_a_token_for_alpha_online_operations() {
        let missing = require_alpha_token(None).unwrap_err();
        let short = require_alpha_token(Some("abc")).unwrap_err();
        let long = require_alpha_token(Some("abcdefghijk")).unwrap_err();
        let six_emojis = require_alpha_token(Some("😀😀😀😀😀😀")).unwrap_err();

        assert_eq!(missing.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(short.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(long.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(six_emojis.kind(), io::ErrorKind::InvalidInput);
        assert!(missing.to_string().contains("--token"));
    }

    #[test]
    fn parses_kernel_store_with_an_overridden_release_name() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "kernel",
            "store",
            "download.iso",
            "--name",
            "Edgeless_Beta_Ofial_4.1.0_2.iso",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Kernel {
                command: KernelCommand::Store { path, name: Some(name) }
            } if path == std::path::Path::new("download.iso")
                && name == "Edgeless_Beta_Ofial_4.1.0_2.iso"
        ));
    }
}
