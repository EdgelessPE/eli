use clap::Subcommand;
use eli_lib::Ctx;
use eli_lib::command::nespak::LoadStatus;
use std::io;

#[derive(Debug, Subcommand)]
pub(crate) enum NesPakCommand {
    /// Import the built-in NesPak archive into the running Edgeless environment.
    Load,
}

pub(crate) fn execute(ctx: &Ctx, command: NesPakCommand) -> io::Result<()> {
    match command {
        NesPakCommand::Load => match eli_lib::command::nespak::load(ctx)? {
            LoadStatus::Loaded => {
                println!("Imported NesPak and loaded Nes.ini");
                Ok(())
            }
            LoadStatus::Extracted => {
                println!("Imported NesPak; Nes.ini was not found");
                Ok(())
            }
        },
    }
}
