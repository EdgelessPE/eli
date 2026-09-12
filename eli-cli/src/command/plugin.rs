use super::warn_automatic_bootdisk_selection;
use clap::{Subcommand, ValueEnum};
use eli_lib::Ctx;
use eli_lib::command::plugin::localboost::clean::CleanTarget;
use eli_lib::command::plugin::{LoadOptions, LoadStatus, LocalBoostHandling, PluginAttribute};
use std::ffi::OsString;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const NAME_COLUMN_WIDTH: usize = 32;
const VERSION_COLUMN_WIDTH: usize = 16;
const AUTHOR_COLUMN_WIDTH: usize = 16;
const ATTRIBUTE_COLUMN_WIDTH: usize = 16;

#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    /// List plugin packages and their attributes.
    List,
    /// Change a plugin package attribute.
    Attr {
        #[arg(value_name = "PLUGIN")]
        plugin: OsString,
        #[arg(value_enum, ignore_case = true, value_name = "ATTRIBUTE")]
        attribute: PluginAttributeArg,
    },
    /// Delete a plugin package by its file name or file stem.
    Delete {
        #[arg(value_name = "PLUGIN")]
        plugin: OsString,
    },
    /// Store a plugin package on the boot disk.
    Store {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Move a plugin package into the outdated package directory.
    Outdate {
        #[arg(value_name = "PLUGIN")]
        plugin: OsString,
    },
    /// Load one or more plugin packages into the running Edgeless environment.
    Load {
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Open the graphical plugin loader.
        #[arg(long)]
        gui: bool,
        #[arg(short = 'r', long)]
        recursive: bool,
        #[arg(short = 'j', long, default_value_t = 2, value_name = "COUNT")]
        jobs: usize,
        #[arg(long, value_enum, ignore_case = true, default_value_t = LocalBoostArg::Ignore)]
        localboost: LocalBoostArg,
    },
    /// Manage LocalBoost plugin packages.
    Localboost {
        #[command(subcommand)]
        command: LocalBoostCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LocalBoostCommand {
    /// Install and load a package through the selected LocalBoost repository.
    Load {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Load every existing unit from the selected repository.
    Startup,
    /// Remove one LocalBoost unit or explicitly clear every repository.
    Clean {
        /// Repository directory name of the plugin to remove.
        #[arg(
            value_name = "PLUGIN",
            required_unless_present = "all",
            conflicts_with = "all"
        )]
        plugin: Option<OsString>,
        /// Remove all LocalBoost repositories on this computer.
        #[arg(long, required_unless_present = "plugin", conflicts_with = "plugin")]
        all: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum PluginAttributeArg {
    #[value(name = "Normal")]
    Normal,
    #[value(name = "Frozen")]
    Frozen,
    #[value(name = "LocalBoost")]
    LocalBoost,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum LocalBoostArg {
    #[value(name = "ignore")]
    Ignore,
    #[value(name = "load")]
    Load,
}

impl From<LocalBoostArg> for LocalBoostHandling {
    fn from(value: LocalBoostArg) -> Self {
        match value {
            LocalBoostArg::Ignore => Self::Ignore,
            LocalBoostArg::Load => Self::Load,
        }
    }
}

impl From<PluginAttributeArg> for PluginAttribute {
    fn from(attribute: PluginAttributeArg) -> Self {
        match attribute {
            PluginAttributeArg::Normal => Self::Normal,
            PluginAttributeArg::Frozen => Self::Frozen,
            PluginAttributeArg::LocalBoost => Self::LocalBoost,
        }
    }
}

pub(crate) fn execute(ctx: Arc<Ctx>, command: PluginCommand) -> io::Result<()> {
    match command {
        PluginCommand::List => list(ctx.as_ref()),
        PluginCommand::Attr { plugin, attribute } => attr(ctx.as_ref(), &plugin, attribute.into()),
        PluginCommand::Delete { plugin } => delete(ctx.as_ref(), &plugin),
        PluginCommand::Store { path } => store(ctx.as_ref(), &path),
        PluginCommand::Outdate { plugin } => outdate(ctx.as_ref(), &plugin),
        PluginCommand::Load {
            paths,
            gui,
            recursive,
            jobs,
            localboost,
        } => load(ctx, paths, gui, recursive, jobs, localboost),
        PluginCommand::Localboost { command } => match command {
            LocalBoostCommand::Load { path } => localboost_load(ctx.as_ref(), &path),
            LocalBoostCommand::Startup => localboost_startup(ctx.as_ref()),
            LocalBoostCommand::Clean { plugin, all } => {
                let target = if all {
                    CleanTarget::All
                } else {
                    CleanTarget::Plugin(plugin.expect("Clap requires PLUGIN or --all"))
                };
                localboost_clean(ctx.as_ref(), target)
            }
        },
    }
}

fn localboost_load(ctx: &Ctx, path: &Path) -> io::Result<()> {
    let status = with_repository_selection(ctx, || {
        eli_lib::command::plugin::localboost::load::load(ctx, path)
    })?;
    match status {
        eli_lib::command::plugin::localboost::load::LoadStatus::Loaded => {
            println!("Loaded with LocalBoost {}", path.display())
        }
        eli_lib::command::plugin::localboost::load::LoadStatus::LoadedWithCompatibilityWarning => {
            println!("Loaded with LocalBoost {}", path.display());
            print_localboost_compatibility_warning(path.as_os_str());
        }
        eli_lib::command::plugin::localboost::load::LoadStatus::AlreadyLoaded => {
            println!("Already loaded with LocalBoost {}", path.display())
        }
    }
    Ok(())
}

fn localboost_startup(ctx: &Ctx) -> io::Result<()> {
    let summary = with_repository_selection(ctx, || {
        eli_lib::command::plugin::localboost::startup::startup(ctx)
    })?;
    for result in &summary.results {
        match &result.result {
            Ok(eli_lib::command::plugin::localboost::load::LoadStatus::Loaded) => {
                println!("Loaded LocalBoost unit {}", result.plugin.to_string_lossy())
            }
            Ok(
                eli_lib::command::plugin::localboost::load::LoadStatus::LoadedWithCompatibilityWarning,
            ) => {
                println!("Loaded LocalBoost unit {}", result.plugin.to_string_lossy());
                print_localboost_compatibility_warning(&result.plugin);
            }
            Ok(eli_lib::command::plugin::localboost::load::LoadStatus::AlreadyLoaded) => println!(
                "Already loaded LocalBoost unit {}",
                result.plugin.to_string_lossy()
            ),
            Err(error) => eprintln!(
                "Failed LocalBoost unit {}: {error}",
                result.plugin.to_string_lossy()
            ),
        }
    }
    println!(
        "{} loaded, {} already loaded, {} failed",
        summary.loaded(),
        summary.already_loaded(),
        summary.failed()
    );
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} LocalBoost unit(s) failed during startup",
            summary.failed()
        )))
    }
}

fn print_localboost_compatibility_warning(plugin: &std::ffi::OsStr) {
    eprintln!(
        "warning: LocalBoost plugin {} contains BAT/CMD files in a dependency directory and may not work correctly",
        plugin.to_string_lossy()
    );
}

fn localboost_clean(ctx: &Ctx, target: CleanTarget) -> io::Result<()> {
    let summary = with_repository_selection(ctx, || {
        eli_lib::command::plugin::localboost::clean::clean(ctx, target.clone())
    })?;
    for result in &summary.results {
        let label = result
            .plugin
            .as_ref()
            .map(|plugin| plugin.to_string_lossy().into_owned())
            .unwrap_or_else(|| result.repository.display().to_string());
        match &result.result {
            Ok(()) => println!("Cleaned LocalBoost {label}"),
            Err(error) => eprintln!("Failed to clean LocalBoost {label}: {error}"),
        }
    }
    if summary.requires_restart() {
        eprintln!(
            "LocalBoost script side effects cannot be fully reversed; restart Windows PE to complete cleanup."
        );
    }
    if summary.runtime_cleanup_incomplete() {
        eprintln!("Some current-session LocalBoost files could not be safely removed.");
    }
    println!(
        "{} cleaned, {} failed",
        summary.succeeded(),
        summary.failed()
    );
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} LocalBoost cleanup operation(s) failed",
            summary.failed()
        )))
    }
}

fn with_repository_selection<T>(
    ctx: &Ctx,
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let Some(required) =
                    eli_lib::command::plugin::localboost::repository::selection_required(&error)
                else {
                    return Err(error);
                };
                #[cfg(windows)]
                {
                    let selected = crate::ui::plugin::localboost_repository::select(
                        required.candidates().to_vec(),
                    )?
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::Interrupted,
                            "LocalBoost repository selection was cancelled",
                        )
                    })?;
                    eli_lib::command::plugin::localboost::repository::confirm(ctx, &selected)?;
                }
                #[cfg(not(windows))]
                {
                    let _ = required;
                    return Err(error);
                }
            }
        }
    }
}

fn load(
    ctx: Arc<Ctx>,
    paths: Vec<PathBuf>,
    gui: bool,
    recursive: bool,
    jobs: usize,
    localboost: LocalBoostArg,
) -> io::Result<()> {
    let paths = normalize_load_paths(paths);
    let jobs = NonZeroUsize::new(jobs).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin load job count must be greater than zero",
        )
    })?;
    let options = LoadOptions {
        recursive,
        jobs,
        local_boost: localboost.into(),
        on_inputs_expanded: None,
        on_progress: None,
    };
    if gui {
        #[cfg(windows)]
        return crate::ui::plugin::load::run(ctx, paths, options);
        #[cfg(not(windows))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the plugin load GUI is only available on Windows",
        ));
    }
    let summary = with_repository_selection(ctx.as_ref(), || {
        eli_lib::command::plugin::load(ctx.as_ref(), &paths, options.clone())
    })?;
    for result in &summary.results {
        match &result.result {
            Ok(LoadStatus::Loaded) => println!("Loaded {}", result.path.display()),
            Ok(LoadStatus::LoadedWithLocalBoost) => {
                println!("Loaded with LocalBoost {}", result.path.display())
            }
            Ok(LoadStatus::LoadedWithLocalBoostCompatibilityWarning) => {
                println!("Loaded with LocalBoost {}", result.path.display());
                print_localboost_compatibility_warning(result.path.as_os_str());
            }
            Ok(LoadStatus::AlreadyLoadedWithLocalBoost) => {
                println!("Already loaded with LocalBoost {}", result.path.display())
            }
            Ok(LoadStatus::SkippedLocalBoost) => {
                println!("Skipped LocalBoost package {}", result.path.display())
            }
            Err(error) => eprintln!("Failed {}: {error}", result.path.display()),
        }
    }
    println!(
        "{} succeeded, {} failed, {} skipped",
        summary.succeeded(),
        summary.failed(),
        summary.skipped()
    );
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} plugin package(s) failed to load",
            summary.failed()
        )))
    }
}

/// 清除 Windows 命令解释器可能遗留在路径两端的包裹引号。
fn normalize_load_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| PathBuf::from(path.to_string_lossy().trim_matches('"')))
        .collect()
}

fn list(ctx: &Ctx) -> io::Result<()> {
    warn_automatic_bootdisk_selection(ctx.bootdisk()?);
    let plugins = eli_lib::command::plugin::list(ctx)?;
    println!(
        "{:<NAME_COLUMN_WIDTH$}{:<VERSION_COLUMN_WIDTH$}{:<AUTHOR_COLUMN_WIDTH$}{:<ATTRIBUTE_COLUMN_WIDTH$}AutoBuild",
        "Name", "Version", "Author", "Attribute",
    );
    for plugin in plugins {
        println!(
            "{:<NAME_COLUMN_WIDTH$}{:<VERSION_COLUMN_WIDTH$}{:<AUTHOR_COLUMN_WIDTH$}{:<ATTRIBUTE_COLUMN_WIDTH$}{}",
            plugin.name,
            plugin.version,
            plugin.author,
            plugin.attribute,
            if plugin.automatically_built {
                "Yes"
            } else {
                "No"
            },
        );
    }
    Ok(())
}

fn attr(ctx: &Ctx, plugin: &std::ffi::OsStr, attribute: PluginAttribute) -> io::Result<()> {
    let changed = eli_lib::command::plugin::set_attribute(ctx, plugin, attribute)?;
    println!("Changed {} to {attribute}", changed.display());
    Ok(())
}

fn delete(ctx: &Ctx, plugin: &std::ffi::OsStr) -> io::Result<()> {
    let deleted = eli_lib::command::plugin::delete(ctx, plugin)?;
    println!("Deleted {}", deleted.display());
    Ok(())
}

fn store(ctx: &Ctx, path: &Path) -> io::Result<()> {
    let stored = eli_lib::command::plugin::store(ctx, path)?;
    println!("Stored {}", stored.display());
    Ok(())
}

fn outdate(ctx: &Ctx, plugin: &std::ffi::OsStr) -> io::Result<()> {
    let moved = eli_lib::command::plugin::outdate(ctx, plugin)?;
    println!("Outdated {}", moved.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_load_paths_removes_command_line_wrapper_quotes() {
        let paths = normalize_load_paths(vec![
            PathBuf::from("\"C:\\Edgeless\\Resource\""),
            PathBuf::from("C:\\Edgeless\\Resource\""),
        ]);

        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:\\Edgeless\\Resource"),
                PathBuf::from("C:\\Edgeless\\Resource"),
            ]
        );
    }
}
