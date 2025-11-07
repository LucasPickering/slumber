use crate::{
    GlobalArgs, Subcommand,
    util::{edit_and_validate, print_yaml},
};
use clap::Parser;
use slumber_config::Config;
use std::process::ExitCode;

/// View and edit the active Slumber request collection file
///
/// By default, this will use auto-detection to find the collection file. Use
/// the global --file/-f argument to view/edit a specific collection file. This
/// argument must be passed *before* the `collection` subcommand:
///
///   slumber --file my-collection.yml collection
///
/// See the --file for a description of the auto-detection logic.
#[derive(Clone, Debug, Parser)]
#[clap(verbatim_doc_comment)]
pub struct CollectionCommand {
    /// Open the configuration file in the default editor
    ///
    /// Configure which editor to use:
    /// https://slumber.lucaspickering.me/user_guide/tui/editor.html#editing
    #[clap(long)]
    #[expect(rustdoc::bare_urls)]
    edit: bool,
    /// Print the path of the collection file and exit; overrides all other
    /// arguments
    #[clap(long)]
    path: bool,
}

impl Subcommand for CollectionCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let collection_file = global.collection_file()?;
        if self.path {
            println!("{collection_file}");
            Ok(ExitCode::SUCCESS)
        } else if self.edit {
            let config = Config::load()?;
            edit_and_validate(&config, collection_file.path(), || {
                collection_file.load()
            })
        } else {
            let collection = collection_file.load()?;
            print_yaml(&collection)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
