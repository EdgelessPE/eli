use super::warn_automatic_bootdisk_selection;
use clap::{Subcommand, ValueEnum};
use eli_lib::Ctx;
use eli_lib::command::theme::{
    ApplySummary, ComponentStatus, StoreSummary, ThemeComponent, ThemeDeleteTarget,
};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Subcommand)]
pub(crate) enum ThemeCommand {
    /// List theme resources configured on the selected boot disk.
    List,
    /// Delete one theme resource or every configured theme resource from the selected boot disk.
    Delete {
        #[arg(value_enum, ignore_case = true, value_name = "RESOURCE")]
        resource: ThemeResourceArg,
    },
    /// Apply a theme package, resource pack or wallpaper to the current Edgeless PE session.
    Apply {
        #[arg(value_name = "PACKAGE")]
        package: PathBuf,
    },
    /// Store a theme package, resource pack or wallpaper on the selected boot disk.
    Store {
        #[arg(value_name = "PACKAGE")]
        package: PathBuf,
    },
    /// Apply the boot disk's default theme before Explorer, or reconcile shortcut icons later.
    Startup {
        /// Only reconcile published EIS icons with shortcuts created after Explorer startup.
        #[arg(long)]
        reconcile: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ThemeResourceArg {
    #[value(name = "icon", alias = "eis")]
    Icon,
    #[value(name = "system-icon", alias = "ess")]
    SystemIcon,
    #[value(name = "loadscreen", alias = "els")]
    LoadScreen,
    #[value(name = "mouse", alias = "ems")]
    Mouse,
    #[value(name = "start-menu", alias = "esc")]
    StartMenu,
    #[value(name = "wallpaper", alias = "jpg")]
    Wallpaper,
    #[value(name = "all")]
    All,
}

impl From<ThemeResourceArg> for ThemeDeleteTarget {
    fn from(resource: ThemeResourceArg) -> Self {
        let component = match resource {
            ThemeResourceArg::Icon => ThemeComponent::IconPack,
            ThemeResourceArg::SystemIcon => ThemeComponent::SystemIconPack,
            ThemeResourceArg::LoadScreen => ThemeComponent::LoadScreen,
            ThemeResourceArg::Mouse => ThemeComponent::MouseStyle,
            ThemeResourceArg::StartMenu => ThemeComponent::StartIsBackConfig,
            ThemeResourceArg::Wallpaper => ThemeComponent::Wallpaper,
            ThemeResourceArg::All => return Self::All,
        };
        Self::Resource(component)
    }
}

pub(crate) fn execute(ctx: Arc<Ctx>, command: ThemeCommand) -> io::Result<()> {
    match command {
        ThemeCommand::List => list(ctx.as_ref()),
        ThemeCommand::Delete { resource } => delete(ctx.as_ref(), resource.into()),
        ThemeCommand::Apply { package } => apply(ctx.as_ref(), &package),
        ThemeCommand::Store { package } => store(ctx.as_ref(), &package),
        ThemeCommand::Startup { reconcile } => startup(ctx.as_ref(), reconcile),
    }
}

fn list(ctx: &Ctx) -> io::Result<()> {
    warn_automatic_bootdisk_selection(ctx.bootdisk()?);
    let summary = eli_lib::command::theme::list(ctx)?;
    println!("Boot disk: {}", summary.bootdisk.display());
    println!("{:<16}Configured", "Resource");
    for entry in summary.resources {
        println!(
            "{:<16}{}",
            resource_label(entry.resource),
            if entry.configured { "Yes" } else { "No" }
        );
    }
    Ok(())
}

fn delete(ctx: &Ctx, target: ThemeDeleteTarget) -> io::Result<()> {
    let summary = eli_lib::command::theme::delete(ctx, target)?;
    if summary.deleted.is_empty() {
        println!(
            "No configured theme resources found on {}",
            summary.bootdisk.display()
        );
    } else {
        for resource in summary.deleted {
            println!("Deleted {}", resource_label(resource));
        }
        println!("Updated theme resources on {}", summary.bootdisk.display());
    }
    Ok(())
}

fn resource_label(resource: ThemeComponent) -> &'static str {
    match resource {
        ThemeComponent::IconPack => "Icon Pack",
        ThemeComponent::SystemIconPack => "System Icons",
        ThemeComponent::LoadScreen => "LoadScreen",
        ThemeComponent::MouseStyle => "Mouse Style",
        ThemeComponent::StartIsBackConfig => "Start Menu",
        ThemeComponent::Wallpaper => "Wallpaper",
    }
}

fn store(ctx: &Ctx, package: &Path) -> io::Result<()> {
    let summary = eli_lib::command::theme::store(ctx, package)?;
    print_store_summary(&summary);
    Ok(())
}

fn print_store_summary(summary: &StoreSummary) {
    for component in &summary.stored {
        println!("Stored {}", component.display_name());
    }
    if summary.default_replaced {
        println!("Replaced the boot disk default theme directory");
    }
    if summary.wallpaper_backed_up {
        println!("Backed up the previous wallpaper to wp_backup.jpg");
    }
    println!(
        "Stored {} theme input on {}",
        summary.kind.display_name(),
        summary.bootdisk.display()
    );
}

fn apply(ctx: &Ctx, package: &Path) -> io::Result<()> {
    let summary = eli_lib::command::theme::apply(ctx, package)?;
    print_summary(&summary);
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} theme component(s) failed",
            summary.failed()
        )))
    }
}

fn startup(ctx: &Ctx, reconcile: bool) -> io::Result<()> {
    let summary = eli_lib::command::theme::startup(ctx, reconcile)?;
    print_summary(&summary);
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} startup theme component(s) failed",
            summary.failed()
        )))
    }
}

fn print_summary(summary: &ApplySummary) {
    for outcome in &summary.components {
        match &outcome.status {
            ComponentStatus::Applied => {
                println!("Applied {}", outcome.component.display_name());
            }
            ComponentStatus::AppliedWithWarnings(warnings) => {
                println!(
                    "Applied {} (with warnings)",
                    outcome.component.display_name()
                );
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
            }
            ComponentStatus::Skipped => {
                println!("Skipped {}", outcome.component.display_name());
            }
            ComponentStatus::Failed(error) => {
                let phase = if outcome.component
                    == eli_lib::command::theme::ThemeComponent::SystemIconPack
                {
                    "refresh"
                } else {
                    "commit"
                };
                eprintln!(
                    "Failed {} from {} during {phase}: {error}",
                    outcome.component.display_name(),
                    summary.source.display()
                );
            }
        }
    }
    for warning in &summary.warnings {
        eprintln!("warning: {warning}");
    }
    let eis = &summary.eis;
    if eis.checked > 0
        || eis.updated > 0
        || eis.unchanged > 0
        || eis.not_found > 0
        || eis.failed > 0
    {
        println!(
            "Shortcut icons: {} checked, {} updated, {} unchanged, {} unmatched, {} failed",
            eis.checked, eis.updated, eis.unchanged, eis.not_found, eis.failed
        );
    }
    let refresh = &summary.refresh;
    let mut refresh_parts = Vec::new();
    if refresh.explorer_restarted {
        refresh_parts.push("explorer restarted".to_owned());
    }
    if refresh.icon_cache_invalidated {
        refresh_parts.push("icon cache invalidated".to_owned());
    }
    if refresh.cursors_refreshed {
        refresh_parts.push("cursors refreshed".to_owned());
    }
    if refresh.shortcut_notified > 0 {
        refresh_parts.push(format!(
            "{} shortcut(s) notified",
            refresh.shortcut_notified
        ));
    }
    for warning in &refresh.warnings {
        eprintln!("warning: {warning}");
    }
    if !refresh_parts.is_empty() {
        println!("Shell refresh: {}", refresh_parts.join(", "));
    }
    println!(
        "{} applied, {} applied with warnings, {} skipped, {} failed",
        summary.applied(),
        summary.applied_with_warnings(),
        summary.skipped(),
        summary.failed()
    );
}
