mod command;
#[cfg(windows)]
mod ui;

use clap::{Parser, Subcommand};
use command::bootdisk::BootdiskCommand;
use command::config::ConfigCommand;
use command::hook::HookCommand;
use command::kernel::KernelCommand;
use command::loadscreen::LoadscreenCommand;
use command::nespak::NesPakCommand;
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
    /// Manage Edgeless lifecycle hooks.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// Inspect Edgeless kernel versions.
    Kernel {
        #[command(subcommand)]
        command: KernelCommand,
    },
    /// Manage the Edgeless loading screen.
    Loadscreen {
        #[command(subcommand)]
        command: LoadscreenCommand,
    },
    /// Import NesPak resources into the running Edgeless environment.
    Nespak {
        #[command(subcommand)]
        command: NesPakCommand,
    },
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let ctx = Arc::new(Ctx::new(cli.bootdisk));

    match cli.command {
        Command::Bootdisk { command } => command::bootdisk::execute(ctx.as_ref(), command),
        Command::Plugin { command } => command::plugin::execute(ctx, command),
        Command::Config { command } => command::config::execute(ctx.as_ref(), command),
        Command::Hook { command } => command::hook::execute(ctx.as_ref(), command),
        Command::Kernel { command } => command::kernel::execute(ctx.as_ref(), command),
        Command::Loadscreen { command } => command::loadscreen::execute(ctx.as_ref(), command),
        Command::Nespak { command } => command::nespak::execute(ctx.as_ref(), command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::hook::{HookCommand, HookPolicyArg, HookStageArg};
    use crate::command::kernel::{
        KernelAlphaCommand, KernelAlphaVersionCommand, KernelVersionCommand,
    };
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
    fn parses_nespak_load_with_a_component_archive() {
        let cli = Cli::try_parse_from(["eli", "nespak", "load", "NesPak.7z"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Nespak {
                command: NesPakCommand::Load { path }
            } if path == Path::new("NesPak.7z")
        ));
    }

    #[test]
    fn parses_hook_call_options() {
        let cli = Cli::try_parse_from([
            "eli",
            "hook",
            "call",
            "onBootFinished",
            "--policy",
            "async",
            "--dictionary",
            r"X:\custom-hooks",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Hook {
                command: HookCommand::Call {
                    hook,
                    policy: HookPolicyArg::Async,
                    dictionary: Some(dictionary),
                }
            } if hook == HookStageArg::OnBootFinished
                && dictionary == Path::new(r"X:\custom-hooks")
        ));
    }

    #[test]
    fn hook_call_defaults_to_sync_and_the_runtime_dictionary() {
        let cli = Cli::try_parse_from(["eli", "hook", "call", "onExit"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Hook {
                command: HookCommand::Call {
                    policy: HookPolicyArg::Sync,
                    dictionary: None,
                    ..
                }
            }
        ));
    }

    #[test]
    fn parses_hook_list_add_and_remove() {
        assert!(matches!(
            Cli::try_parse_from(["eli", "hook", "list"])
                .unwrap()
                .command,
            Command::Hook {
                command: HookCommand::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["eli", "hook", "add", "onExit", "save.cmd"])
                .unwrap()
                .command,
            Command::Hook {
                command: HookCommand::Add { hook, script_path }
            } if hook == HookStageArg::OnExit && script_path == Path::new("save.cmd")
        ));
        assert!(matches!(
            Cli::try_parse_from(["eli", "hook", "remove", "onExit", "save.cmd"])
                .unwrap()
                .command,
            Command::Hook {
                command: HookCommand::Remove { hook, script }
            } if hook == HookStageArg::OnExit && script == "save.cmd"
        ));
    }

    #[test]
    fn rejects_undocumented_hook_stages_for_call_add_and_remove() {
        for arguments in [
            vec!["eli", "hook", "call", "customStage"],
            vec!["eli", "hook", "add", "customStage", "save.cmd"],
            vec!["eli", "hook", "remove", "customStage", "save.cmd"],
        ] {
            let error = Cli::try_parse_from(arguments).unwrap_err();
            let rendered = error.to_string();
            assert!(rendered.contains("invalid value 'customStage'"));
            assert!(rendered.contains("onDiskFound"));
            assert!(rendered.contains("onExit"));
        }
    }

    #[test]
    fn requires_a_component_archive_for_nespak_load() {
        assert!(Cli::try_parse_from(["eli", "nespak", "load"]).is_err());
    }

    #[test]
    fn parses_nespak_store_with_a_component_archive() {
        let cli = Cli::try_parse_from(["eli", "nespak", "store", "NesPak.7z"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Nespak {
                command: NesPakCommand::Store { path }
            } if path == Path::new("NesPak.7z")
        ));
    }

    #[test]
    fn requires_a_component_archive_for_nespak_store() {
        assert!(Cli::try_parse_from(["eli", "nespak", "store"]).is_err());
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
    fn parses_localboost_startup() {
        let cli = Cli::try_parse_from(["eli", "plugin", "localboost", "startup"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Plugin {
                command: PluginCommand::Localboost {
                    command: command::plugin::LocalBoostCommand::Startup,
                }
            }
        ));
    }

    #[test]
    fn localboost_clean_requires_exactly_one_target() {
        assert!(Cli::try_parse_from(["eli", "plugin", "localboost", "clean"]).is_err());
        assert!(
            Cli::try_parse_from(["eli", "plugin", "localboost", "clean", "plugin", "--all",])
                .is_err()
        );

        let plugin =
            Cli::try_parse_from(["eli", "plugin", "localboost", "clean", "工具箱_1.0_作者"])
                .unwrap();
        assert!(matches!(
            plugin.command,
            Command::Plugin {
                command: PluginCommand::Localboost {
                    command: command::plugin::LocalBoostCommand::Clean {
                        plugin: Some(name),
                        all: false,
                    },
                }
            } if name == "工具箱_1.0_作者"
        ));

        let all = Cli::try_parse_from(["eli", "plugin", "localboost", "clean", "--all"]).unwrap();
        assert!(matches!(
            all.command,
            Command::Plugin {
                command: PluginCommand::Localboost {
                    command: command::plugin::LocalBoostCommand::Clean {
                        plugin: None,
                        all: true,
                    },
                }
            }
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
    fn parses_each_kernel_version_source() {
        for (source, expected) in [
            ("current", KernelVersionCommand::Current),
            ("latest", KernelVersionCommand::Latest),
            ("bootdisk", KernelVersionCommand::Bootdisk),
        ] {
            let cli = Cli::try_parse_from(["eli", "kernel", "version", source]).unwrap();

            assert!(matches!(
                cli.command,
                Command::Kernel {
                    command: KernelCommand::Version { command },
                } if std::mem::discriminant(&command) == std::mem::discriminant(&expected)
            ));
        }
    }

    #[test]
    fn parses_kernel_alpha_commands() {
        let latest = Cli::try_parse_from([
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
            latest.command,
            Command::Kernel {
                command: KernelCommand::Alpha {
                    token: Some(_),
                    command: KernelAlphaCommand::Version {
                        command: KernelAlphaVersionCommand::Latest,
                    },
                },
            }
        ));

        let bootdisk =
            Cli::try_parse_from(["eli", "kernel", "alpha", "version", "bootdisk"]).unwrap();
        assert!(matches!(
            bootdisk.command,
            Command::Kernel {
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
    fn parses_kernel_download_with_an_explicit_directory() {
        let cli =
            Cli::try_parse_from(["eli", "kernel", "download", "--directory", "/tmp/iso"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Kernel {
                command: KernelCommand::Download {
                    directory,
                    force: false,
                },
            } if directory == Path::new("/tmp/iso")
        ));
    }

    #[test]
    fn parses_kernel_download_force_overwrite() {
        let cli =
            Cli::try_parse_from(["eli", "kernel", "download", "-d", "/tmp/iso", "-f"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Kernel {
                command: KernelCommand::Download { force: true, .. },
            }
        ));
    }

    #[cfg(not(feature = "loadscreen-bake"))]
    #[test]
    fn excludes_loadscreen_bake_when_the_feature_is_disabled() {
        let error =
            Cli::try_parse_from(["eli", "loadscreen", "bake", "wallpaper.png", "-d", "baked"])
                .unwrap_err();

        assert!(error.to_string().contains("unrecognized subcommand 'bake'"));
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
