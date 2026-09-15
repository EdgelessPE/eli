use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::command::theme::{ApplySummary, ComponentStatus};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Subcommand)]
pub(crate) enum ThemeCommand {
    /// Apply a theme package, resource pack or wallpaper to the current Edgeless PE session.
    Apply {
        #[arg(value_name = "PACKAGE")]
        package: PathBuf,
    },
}

pub(crate) fn execute(ctx: Arc<Ctx>, command: ThemeCommand) -> io::Result<()> {
    match command {
        ThemeCommand::Apply { package } => apply(ctx.as_ref(), &package),
    }
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
                eprintln!(
                    "Failed {} from {} during apply: {error}",
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
    if eis.updated > 0 || eis.not_found > 0 || eis.failed > 0 {
        println!(
            "Shortcut icons: {} updated, {} unmatched, {} failed",
            eis.updated, eis.not_found, eis.failed
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
