use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::command::nespak::LoadStatus;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub(crate) enum NesPakCommand {
    /// Import a NesPak component archive into the running Edgeless environment.
    Load {
        /// Path to the NesPak component archive.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

pub(crate) fn execute(ctx: &Ctx, command: NesPakCommand) -> io::Result<()> {
    match command {
        NesPakCommand::Load { path } => match eli_lib::command::nespak::load(ctx, &path)? {
            LoadStatus::Loaded => {
                println!("Imported NesPak from {} and loaded Nes.ini", path.display());
                Ok(())
            }
            LoadStatus::Extracted => {
                println!(
                    "Imported NesPak from {}; Nes.ini was not found",
                    path.display()
                );
                Ok(())
            }
        },
    }
}
