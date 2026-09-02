use super::warn_automatic_bootdisk_selection;
use clap::{Subcommand, ValueEnum};
use eli_lib::Ctx;
use eli_lib::command::plugin::PluginAttribute;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

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

impl From<PluginAttributeArg> for PluginAttribute {
    fn from(attribute: PluginAttributeArg) -> Self {
        match attribute {
            PluginAttributeArg::Normal => Self::Normal,
            PluginAttributeArg::Frozen => Self::Frozen,
            PluginAttributeArg::LocalBoost => Self::LocalBoost,
        }
    }
}

pub(crate) fn execute(ctx: &Ctx, command: PluginCommand) -> io::Result<()> {
    match command {
        PluginCommand::List => list(ctx),
        PluginCommand::Attr { plugin, attribute } => attr(ctx, &plugin, attribute.into()),
        PluginCommand::Delete { plugin } => delete(ctx, &plugin),
        PluginCommand::Store { path } => store(ctx, &path),
        PluginCommand::Outdate { plugin } => outdate(ctx, &plugin),
    }
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
