//! Utilties for the script(s) that generate documentation. This is in its own
//! crate rather than just being in the script because rust-analyzer can't parse
//! scripts so it doesn't provide any features. Coding alone is scary...

#![forbid(unsafe_code)]
#![deny(clippy::all)]

mod template_functions;

use anyhow::anyhow;
use clap::{Parser, ValueEnum};
use mdbook_preprocessor::{
    Preprocessor, PreprocessorContext,
    book::{Book, BookItem},
    errors::{Error, Result},
    parse_input,
};
use std::{io, io::IsTerminal};

const NAME: &str = "mdbook-slumber";

#[derive(Debug, Parser)]
#[clap(author, version, about, name = NAME)]
struct Args {
    /// Print debug output instead of running as an mdbook preprocessor
    #[arg(long)]
    debug: bool,
    /// Type of documentation to generate
    #[arg(long)]
    mode: SlumberPreprocessor,
    #[command(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(Clone, Debug, clap::Subcommand)]
enum Subcommand {
    /// Check if the preprocessor supports a particular renderer
    ///
    /// <https://rust-lang.github.io/mdBook/for_developers/preprocessors.html#hooking-into-mdbook>
    Supports { renderer: String },
}

/// Run the CLI utility, which is meant to be used as an mdbook preprocessor
pub fn mdbook() -> anyhow::Result<()> {
    let args = Args::parse();
    let preprocessor = args.mode;

    if let Some(Subcommand::Supports { renderer }) = args.subcommand {
        // Caller wants to know if this renderer is supported
        let supported = preprocessor.supports_renderer(&renderer)?;

        // Signal whether the renderer is supported by exiting with 1 or 0.
        if supported {
            Ok(())
        } else {
            Err(anyhow!("Unsupported renderer {renderer}"))
        }
    } else if args.debug {
        let markdown = preprocessor.render()?;
        println!("{markdown}");
        Ok(())
    } else {
        if io::stdout().is_terminal() {
            // It's possible the user is testing the preprocessor and meant to
            // do this, but it's probably a mistake. We'll block forever on
            // stdin so given them a warning
            eprintln!(
                "WARNING: Running as mdbook preprocessor, loading from stdin"
            );
        }

        // Run as mdbook preprocessor
        preprocessor.preprocess()
    }
}

/// Preprocessor for the various use cases we support
#[derive(Copy, Clone, Debug, ValueEnum)]
enum SlumberPreprocessor {
    /// Generate markdown for template functions, based on their Rust
    /// function signatures. This will replace any occurrences of the string
    /// `{{#template_functions}}` with the generated markdown.
    TemplateFunctions,
    /// Replace `{{#version}}` with the crate version
    Version,
}

impl SlumberPreprocessor {
    fn preprocess(self) -> Result<()> {
        let (ctx, book) = parse_input(io::stdin())?;
        let processed_book = self.run(&ctx, book)?;
        serde_json::to_writer(io::stdout(), &processed_book)?;
        Ok(())
    }

    /// Render markdown content
    fn render(self) -> Result<String> {
        match self {
            Self::TemplateFunctions => template_functions::render(),
            Self::Version => Ok(env!("CARGO_PKG_VERSION").to_owned()),
        }
    }
}

impl Preprocessor for SlumberPreprocessor {
    fn name(&self) -> &str {
        NAME
    }

    fn run(
        &self,
        _ctx: &PreprocessorContext,
        mut book: Book,
    ) -> Result<Book, Error> {
        let markdown = self.render()?;

        match self {
            Self::TemplateFunctions => {
                book.for_each_mut(|item: &mut BookItem| {
                    if let BookItem::Chapter(chapter) = item {
                        chapter.content = chapter
                            .content
                            .replace(template_functions::REPLACE, &markdown);
                    }
                });
            }
            Self::Version => book.for_each_mut(|item: &mut BookItem| {
                if let BookItem::Chapter(chapter) = item {
                    chapter.content =
                        chapter.content.replace("{{#version}}", &markdown);
                }
            }),
        }
        Ok(book)
    }

    fn supports_renderer(&self, _renderer: &str) -> Result<bool, Error> {
        Ok(true)
    }
}
