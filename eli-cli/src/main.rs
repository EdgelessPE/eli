mod command;

use clap::{Parser, Subcommand};
use command::bootdisk::BootdiskCommand;
use command::config::ConfigCommand;
use command::plugin::PluginCommand;
use eli_lib::Ctx;
use std::path::PathBuf;
use std::sync::Arc;

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
    /// Manage configuration stored on an Edgeless boot disk.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let ctx = Arc::new(Ctx::new(cli.bootdisk));

    match cli.command {
        Command::Bootdisk { command } => command::bootdisk::execute(ctx.as_ref(), command),
        Command::Plugin { command } => command::plugin::execute(ctx, command),
        Command::Config { command } => command::config::execute(ctx.as_ref(), command),
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

    #[test]
    fn parses_plugin_load_with_multiple_inputs_and_options() {
        let cli = Cli::try_parse_from([
            "eli",
            "plugin",
            "load",
            "--jobs",
            "4",
            "--recursive",
            "--localboost",
            "load",
            r"D:\插件包",
            r"E:\工具_1.0_Edgeless.7z",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Load {
                    paths,
                    gui: false,
                    recursive: true,
                    jobs: 4,
                    localboost: command::plugin::LocalBoostArg::Load,
                }
            } if paths.len() == 2
        ));
    }

    #[test]
    fn plugin_load_defaults_to_two_jobs_and_ignores_localboost() {
        let cli = Cli::try_parse_from(["eli", "plugin", "load", "plugin.7z"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Load {
                    jobs: 2,
                    gui: false,
                    recursive: false,
                    localboost: command::plugin::LocalBoostArg::Ignore,
                    ..
                }
            }
        ));
    }

    #[test]
    fn parses_plugin_load_gui_with_paths() {
        let cli = Cli::try_parse_from([
            "eli",
            "plugin",
            "load",
            "--gui",
            r"D:\插件包",
            r"E:\工具_1.0_Edgeless.7z",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Load {
                    paths,
                    gui: true,
                    ..
                }
            } if paths.len() == 2
        ));
    }

    #[test]
    fn rejects_plugin_load_gui_without_paths() {
        assert!(Cli::try_parse_from(["eli", "plugin", "load", "--gui"]).is_err());
    }

    #[test]
    fn parses_localboost_load_as_an_independent_command() {
        let cli =
            Cli::try_parse_from(["eli", "plugin", "localboost", "load", r"D:\plugin.7zl"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Localboost {
                    command: command::plugin::LocalBoostCommand::Load { path },
                }
            } if path == Path::new(r"D:\plugin.7zl")
        ));
    }

    #[test]
    fn parses_config_set_with_a_resolution_value() {
        let cli =
            Cli::try_parse_from(["eli", "config", "set", "resolution", "w1920 h1080 b32 f60"])
                .unwrap();

        assert!(matches!(
            cli.command,
            Command::Config {
                command:
                    ConfigCommand::Set {
                        key,
                        value,
                        skip_resolution_validation: false,
                    }
            } if key == "resolution" && value == "w1920 h1080 b32 f60"
        ));
    }

    #[test]
    fn parses_the_resolution_validation_override() {
        let cli = Cli::try_parse_from([
            "eli",
            "config",
            "set",
            "resolution",
            "w1080 h1920 b32 f60",
            "--skip-resolution-validation",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Config {
                command: ConfigCommand::Set {
                    skip_resolution_validation: true,
                    ..
                }
            }
        ));
    }
}
