mod command;

use clap::{Parser, Subcommand};
use command::bootdisk::BootdiskCommand;
use command::plugin::PluginCommand;
use eli_lib::Ctx;
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
    /// Manage plugin packages on an Edgeless boot disk.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let ctx = Ctx::new(cli.bootdisk);

    match cli.command {
        Command::Bootdisk { command } => command::bootdisk::execute(&ctx, command),
        Command::Plugin { command } => command::plugin::execute(&ctx, command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::PluginAttributeArg;
    use std::path::Path;

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
    fn parses_plugin_delete_with_a_complete_file_name() {
        let cli =
            Cli::try_parse_from(["eli", "plugin", "delete", "搜狗拼音_16.4.0.0_Cno（bot）.7z"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Delete { plugin }
            } if plugin == "搜狗拼音_16.4.0.0_Cno（bot）.7z"
        ));
    }

    #[test]
    fn parses_plugin_delete_with_a_stem_and_global_bootdisk() {
        let cli = Cli::try_parse_from([
            "eli",
            "plugin",
            "delete",
            "搜狗拼音_16.4.0.0_Cno（bot）",
            "--bootdisk",
            "/media/Edgeless",
        ])
        .unwrap();

        assert_eq!(cli.bootdisk, Some(PathBuf::from("/media/Edgeless")));
    }

    #[test]
    fn parses_plugin_list() {
        let cli = Cli::try_parse_from(["eli", "plugin", "list"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::List
            }
        ));
    }

    #[test]
    fn parses_plugin_attr_with_plugin_before_attribute() {
        let cli = Cli::try_parse_from([
            "eli",
            "plugin",
            "attr",
            "搜狗拼音_16.4.0.0_Cno（bot）",
            "LocalBoost",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Attr {
                    plugin,
                    attribute: PluginAttributeArg::LocalBoost,
                }
            } if plugin == "搜狗拼音_16.4.0.0_Cno（bot）"
        ));
    }

    #[test]
    fn parses_plugin_attribute_ignoring_ascii_case() {
        let cli =
            Cli::try_parse_from(["eli", "plugin", "attr", "plugin_1.0_author", "frozen"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Attr {
                    attribute: PluginAttributeArg::Frozen,
                    ..
                }
            }
        ));
    }

    #[test]
    fn parses_plugin_store_with_a_package_path() {
        let cli = Cli::try_parse_from(["eli", "plugin", "store", "/tmp/工具箱_1.0.0_Edgeless.7z"])
            .unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Store { path }
            } if path == Path::new("/tmp/工具箱_1.0.0_Edgeless.7z")
        ));
    }

    #[test]
    fn parses_plugin_outdate_with_a_stem() {
        let cli =
            Cli::try_parse_from(["eli", "plugin", "outdate", "工具箱_1.0.0_Edgeless"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Outdate { plugin }
            } if plugin == "工具箱_1.0.0_Edgeless"
        ));
    }
}
