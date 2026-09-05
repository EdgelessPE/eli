use super::warn_automatic_bootdisk_selection;
use clap::Subcommand;
use eli_lib::Ctx;
use std::io;

const KEY_COLUMN_WIDTH: usize = 28;
const VALUE_COLUMN_WIDTH: usize = 32;

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// List configuration keys, their current values, and version availability.
    List,
    /// Set a boolean config key, wallpaper, resolution, or homepage.
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

pub(crate) fn execute(ctx: &Ctx, command: ConfigCommand) -> io::Result<()> {
    match command {
        ConfigCommand::List => list(ctx),
        ConfigCommand::Set { key, value } => set(ctx, &key, &value),
    }
}

fn list(ctx: &Ctx) -> io::Result<()> {
    warn_automatic_bootdisk_selection(ctx.bootdisk()?);
    println!(
        "{:<KEY_COLUMN_WIDTH$}{:<VALUE_COLUMN_WIDTH$}Available",
        "Key", "Value"
    );
    for entry in eli_lib::command::config::list(ctx)? {
        println!(
            "{:<KEY_COLUMN_WIDTH$}{:<VALUE_COLUMN_WIDTH$}{}",
            entry.key,
            entry.value,
            if entry.available { "Yes" } else { "No" }
        );
    }
    Ok(())
}

fn set(ctx: &Ctx, key: &str, value: &str) -> io::Result<()> {
    let path = eli_lib::command::config::set(ctx, key, value)?;
    println!("Set {key} at {}", path.display());
    Ok(())
}
