use crate::{cli::Subcommand, util::Directory, GlobalArgs};
use async_trait::async_trait;
use clap::Parser;

/// Show meta information about slumber
#[derive(Clone, Debug, Parser)]
pub struct ShowCommand {
    #[command(subcommand)]
    target: ShowTarget,
}

#[derive(Copy, Clone, Debug, clap::Subcommand)]
enum ShowTarget {
    /// Show the directory where slumber stores data and log files
    Dir,
}

#[async_trait]
impl Subcommand for ShowCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<()> {
        match self.target {
            ShowTarget::Dir => println!("{}", Directory::root()),
        }
        Ok(())
    }
}
