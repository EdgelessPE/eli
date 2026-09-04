use super::warn_automatic_bootdisk_selection;
use clap::{Subcommand, ValueEnum};
use eli_lib::Ctx;
use eli_lib::command::plugin::{LoadOptions, LoadStatus, LocalBoostHandling, PluginAttribute};
use std::ffi::OsString;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(windows)]
mod load_gui;

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
        },
    }
}

fn localboost_load(ctx: &Ctx, path: &Path) -> io::Result<()> {
    eli_lib::command::plugin::localboost::load::load(ctx, path)?;
    println!("Loaded with LocalBoost {}", path.display());
    Ok(())
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
        return load_gui::run(ctx, paths, options);
        #[cfg(not(windows))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the plugin load GUI is only available on Windows",
        ));
    }
    let summary = eli_lib::command::plugin::load(ctx.as_ref(), &paths, options)?;
    for result in &summary.results {
        match &result.result {
            Ok(LoadStatus::Loaded) => println!("Loaded {}", result.path.display()),
            Ok(LoadStatus::LoadedWithLocalBoost) => {
                println!("Loaded with LocalBoost {}", result.path.display())
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
