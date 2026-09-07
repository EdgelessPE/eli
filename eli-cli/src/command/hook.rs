use super::warn_automatic_bootdisk_selection;
use clap::{Subcommand, ValueEnum};
use eli_lib::Ctx;
use eli_lib::command::hook::{CallOptions, CallPolicy, HookStage};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

const HOOK_COLUMN_WIDTH: usize = 24;

#[derive(Debug, Subcommand)]
pub(crate) enum HookCommand {
    /// Call all scripts configured for a runtime hook.
    Call {
        #[arg(value_name = "HOOK")]
        hook: HookStageArg,
        /// Choose whether scripts are awaited in order or started concurrently.
        #[arg(long, value_enum, ignore_case = true, default_value_t = HookPolicyArg::Sync)]
        policy: HookPolicyArg,
        /// Override the runtime hook dictionary.
        #[arg(long, visible_alias = "directory", value_name = "PATH")]
        dictionary: Option<PathBuf>,
    },
    /// List all hook scripts configured on the selected boot disk.
    List,
    /// Add a script to a hook on the selected boot disk.
    Add {
        #[arg(value_name = "HOOK")]
        hook: HookStageArg,
        #[arg(value_name = "SCRIPT_PATH")]
        script_path: PathBuf,
    },
    /// Remove a script from a hook on the selected boot disk.
    Remove {
        #[arg(value_name = "HOOK")]
        hook: HookStageArg,
        #[arg(value_name = "SCRIPT")]
        script: OsString,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum HookPolicyArg {
    #[value(name = "sync")]
    Sync,
    #[value(name = "async")]
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum HookStageArg {
    #[value(name = "onDiskFound")]
    OnDiskFound,
    #[value(name = "beforeLocalBoost")]
    BeforeLocalBoost,
    #[value(name = "beforePluginLoading")]
    BeforePluginLoading,
    #[value(name = "onDesktopShown")]
    OnDesktopShown,
    #[value(name = "onBootFinished")]
    OnBootFinished,
    #[value(name = "onExit")]
    OnExit,
}

impl From<HookStageArg> for HookStage {
    fn from(value: HookStageArg) -> Self {
        match value {
            HookStageArg::OnDiskFound => Self::OnDiskFound,
            HookStageArg::BeforeLocalBoost => Self::BeforeLocalBoost,
            HookStageArg::BeforePluginLoading => Self::BeforePluginLoading,
            HookStageArg::OnDesktopShown => Self::OnDesktopShown,
            HookStageArg::OnBootFinished => Self::OnBootFinished,
            HookStageArg::OnExit => Self::OnExit,
        }
    }
}

impl From<HookPolicyArg> for CallPolicy {
    fn from(value: HookPolicyArg) -> Self {
        match value {
            HookPolicyArg::Sync => Self::Sync,
            HookPolicyArg::Async => Self::Async,
        }
    }
}

pub(crate) fn execute(ctx: &Ctx, command: HookCommand) -> io::Result<()> {
    match command {
        HookCommand::Call {
            hook,
            policy,
            dictionary,
        } => call(ctx, hook, policy, dictionary),
        HookCommand::List => list(ctx),
        HookCommand::Add { hook, script_path } => add(ctx, hook, &script_path),
        HookCommand::Remove { hook, script } => remove(ctx, hook, &script),
    }
}

fn call(
    ctx: &Ctx,
    hook: HookStageArg,
    policy: HookPolicyArg,
    dictionary: Option<PathBuf>,
) -> io::Result<()> {
    let mut options = CallOptions {
        policy: policy.into(),
        ..CallOptions::default()
    };
    if let Some(dictionary) = dictionary {
        options.dictionary = dictionary;
    }
    let summary = eli_lib::command::hook::call(ctx, hook.into(), options)?;
    for result in &summary.results {
        match &result.result {
            Ok(()) => println!("Called {}", result.path.display()),
            Err(error) => eprintln!("Failed {}: {error}", result.path.display()),
        }
    }
    println!(
        "{} succeeded, {} failed",
        summary.succeeded(),
        summary.failed()
    );
    if summary.is_success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} hook script(s) failed",
            summary.failed()
        )))
    }
}

fn list(ctx: &Ctx) -> io::Result<()> {
    warn_automatic_bootdisk_selection(ctx.bootdisk()?);
    println!("{:<HOOK_COLUMN_WIDTH$}Script", "Hook");
    for script in eli_lib::command::hook::list(ctx)? {
        println!(
            "{:<HOOK_COLUMN_WIDTH$}{}",
            script.hook.as_str(),
            script.script.to_string_lossy()
        );
    }
    Ok(())
}

fn add(ctx: &Ctx, hook: HookStageArg, script_path: &Path) -> io::Result<()> {
    let path = eli_lib::command::hook::add(ctx, hook.into(), script_path)?;
    println!("Added {}", path.display());
    Ok(())
}

fn remove(ctx: &Ctx, hook: HookStageArg, script: &std::ffi::OsStr) -> io::Result<()> {
    let path = eli_lib::command::hook::remove(ctx, hook.into(), script)?;
    println!("Removed {}", path.display());
    Ok(())
}
