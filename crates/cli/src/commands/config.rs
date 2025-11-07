use crate::{
    GlobalArgs, Subcommand,
    util::{edit_and_validate, print_yaml},
};
use clap::Parser;
use slumber_config::Config;
use std::process::ExitCode;

/// View and edit global Slumber configuration
#[derive(Clone, Debug, Parser)]
pub struct ConfigCommand {
    /// Open the configuration file in the default editor
    ///
    /// Configure which editor to use:
    /// https://slumber.lucaspickering.me/user_guide/tui/editor.html#editing
    #[clap(long)]
    #[expect(rustdoc::bare_urls)]
    edit: bool,
    /// Print the path of the config file and exit; overrides all other
    /// arguments
    #[clap(long)]
    path: bool,
}

impl Subcommand for ConfigCommand {
    async fn execute(self, _global: GlobalArgs) -> anyhow::Result<ExitCode> {
        if self.path {
            let path = Config::path();
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        } else if self.edit {
            // If the config is invalid, the user is probably trying to
            // fix it so we should open anyway
            let config = Config::load().unwrap_or_default();
            let path = Config::path();
            edit_and_validate(&config, &path, Config::load)
        } else {
            let config = Config::load()?;
            print_yaml(&config)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
