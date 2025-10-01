use crate::{
    GlobalArgs, Subcommand,
    completions::{complete_profile, complete_recipe},
};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use clap::{Parser, ValueHint};
use dialoguer::{Input, Password, Select as DialoguerSelect};
use indexmap::IndexMap;
use itertools::Itertools;
use slumber_config::Config;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::{CollectionDatabase, Database},
    http::{
        BuildOptions, Exchange, HttpEngine, RequestRecord, RequestSeed,
        ResponseRecord, StoredRequestError, TriggeredRequestError,
    },
    render::{HttpProvider, Prompt, Prompter, Select, TemplateContext},
    util::MaybeStr,
};
use slumber_util::{ResultTraced, ResultTracedAnyhow};
use std::{
    error::Error,
    fs::OpenOptions,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr,
};
use tracing::warn;

/// Exit code to return when `exit_status` flag is set and the HTTP response has
/// an error status code
const HTTP_ERROR_EXIT_CODE: u8 = 2;

/// Execute a single request, and print its response
#[derive(Clone, Debug, Parser)]
#[clap(aliases=&["req", "rq"])]
pub struct RequestCommand {
    #[clap(flatten)]
    build_request: BuildRequestCommand,

    #[clap(flatten)]
    display: DisplayExchangeCommand,

    /// Just print the generated request, instead of sending it. Triggered
    /// sub-requests will also not be executed. Implies `--verbose`.
    #[clap(long)]
    dry_run: bool,

    /// Set process exit code based on HTTP response status. If the status is
    /// <400, exit code is 0. If it's >=400, exit code is 2.
    #[clap(long)]
    exit_status: bool,

    /// Persist the completed request to Slumber's history database. By
    /// default, CLI-based requests are not persisted. The CLI ignores the
    /// `persist` field in the global configuration and recipe definition; this
    /// flag is the only thing that controls persistence for the CLI.
    ///
    /// If the request triggers any upstream chained requests while building,
    /// those requests will NOT be persisted, regardless of the presence of
    /// this flag.
    #[clap(long)]
    persist: bool,
}

/// A helper for any subcommand that needs to build requests. This handles
/// common args, as well as setting up context for rendering requests
#[derive(Clone, Debug, Parser)]
pub struct BuildRequestCommand {
    /// ID of the recipe to render into a request
    #[clap(add = complete_recipe())]
    recipe_id: RecipeId,

    /// ID of the profile to pull template values from. If omitted and the
    /// collection has default profile defined, use that profile. Otherwise,
    /// profile data will not be available.
    #[clap(
        long = "profile",
        short,
        add = complete_profile(),
    )]
    profile: Option<ProfileId>,

    /// List of key=value template field overrides
    #[clap(
        long = "override",
        short = 'o',
        value_parser = parse_key_val::<String, String>,
        // There's no reasonable way of doing completions on this, so disable
        value_hint = ValueHint::Other,
    )]
    overrides: Vec<(String, String)>,
}

/// Helper for any subcommand that prints exchange (request/response)
/// components. This aims to generally match  the behavior of `curl`, including:
/// - By default, only the response body is printed
/// - `--verbose` enables request and response metadata
/// - Everything other than the response body is printed to stderr
/// - Response body is printed to stdout, but can be redirected with `--output`
#[derive(Clone, Debug, Parser)]
pub struct DisplayExchangeCommand {
    /// Print additional request and response metadata
    #[clap(short, long)]
    verbose: bool,

    /// Write to file instead of stdout
    #[clap(long)]
    output: Option<PathBuf>,
}

impl Subcommand for RequestCommand {
    async fn execute(mut self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        // Don't execute sub-requests in a dry run
        let trigger_dependencies = !self.dry_run;
        let (database, http_engine, seed, template_context) = self
            .build_request
            .build_seed(global, trigger_dependencies)?;
        let ticket = http_engine.build(seed, &template_context).await.map_err(
            |error| {
                // If the build failed because triggered requests are disabled,
                // replace it with a custom error message
                if error.has_trigger_disabled_error() {
                    anyhow::Error::from(error.error).context(
                        "Triggered requests are disabled with `--dry-run`",
                    )
                } else {
                    error.error.into()
                }
            },
        )?;

        if self.dry_run {
            // With --dry-run, we don't do anything unless the verbose flag is
            // enabled. Exiting with no output is confusing so for it enabled
            // here.
            self.display.verbose = true;
            self.display.write_request(ticket.record());
            Ok(ExitCode::SUCCESS)
        } else {
            self.display.write_request(ticket.record());

            // Run the request
            let exchange = ticket.send().await?;
            if self.persist {
                // Error here shouldn't be propagated, just logged
                let _ = database.insert_exchange(&exchange).traced();
            }
            let status = exchange.response.status;

            self.display.write_response(&exchange.response)?;

            if self.exit_status && status.as_u16() >= 400 {
                Ok(ExitCode::from(HTTP_ERROR_EXIT_CODE))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}

impl BuildRequestCommand {
    /// Get all the components needed to build a request for the recipe selected
    /// by this command. The returned values can be used to build the request
    /// in whatever format the consumer needs.
    ///
    /// ## Parameters
    ///
    /// - `global`: Global arguments for the CLI
    /// - `persist`: Will the request/response be persisted in history? This
    ///   needs to be passed here (at build time) because it's baked into the
    ///   HTTP engine's config.
    /// - `trigger_dependencies`: Whether chained requests can be executed if
    ///   their triggers apply
    pub fn build_seed(
        self,
        global: GlobalArgs,
        trigger_dependencies: bool,
    ) -> anyhow::Result<(
        CollectionDatabase,
        HttpEngine,
        RequestSeed,
        TemplateContext,
    )> {
        let collection_file = global.collection_file()?;
        let config = Config::load()?;
        let collection = collection_file.load()?;
        let database = Database::load()?.into_collection(&collection_file)?;
        database.set_name(collection.name.as_deref());
        let http_engine = HttpEngine::new(&config.http);

        // Validate profile ID, so we can provide a good error if it's invalid
        if let Some(profile_id) = &self.profile {
            collection.profiles.get(profile_id).ok_or_else(|| {
                anyhow!(
                    "No profile with ID `{profile_id}`; options are: {}",
                    collection.profiles.keys().format(", ")
                )
            })?;
        }

        // Fall back to default profile if defined in the collection
        let selected_profile = self.profile.or_else(|| {
            let default_profile = collection.default_profile()?;
            Some(default_profile.id.clone())
        });

        // Build the request
        let overrides: IndexMap<_, _> = self.overrides.into_iter().collect();
        let template_context = TemplateContext {
            selected_profile,
            collection: collection.into(),
            http_provider: Box::new(CliHttpProvider {
                database: database.clone(),
                http_engine: http_engine.clone(),
                trigger_dependencies,
            }),
            overrides,
            prompter: Box::new(CliPrompter),
            show_sensitive: true,
            root_dir: collection_file.parent().to_owned(),
            state: Default::default(),
        };
        let seed = RequestSeed::new(self.recipe_id, BuildOptions::default());
        Ok((database, http_engine, seed, template_context))
    }
}

impl DisplayExchangeCommand {
    /// Print request details to stderr
    pub fn write_request(&self, request: &RequestRecord) {
        // The request is entirely hidden unless verbose mode is enabled
        if self.verbose {
            eprintln!(
                "> {} {} {}",
                request.method, request.url, request.http_version
            );
            for (header, value) in &request.headers {
                eprintln!("> {}: {}", header, MaybeStr(value.as_bytes()));
            }
            if let Some(body) = &request.body {
                let text = std::str::from_utf8(body).unwrap_or("<binary>");
                eprintln!("> {text}");
            }
        }
    }

    /// Print response metadata to stderr and write response to the user's
    /// designated output (stdout by default)
    pub fn write_response(
        &self,
        response: &ResponseRecord,
    ) -> anyhow::Result<()> {
        // Print metadata
        if self.verbose {
            eprintln!();
            eprintln!("< {}", response.status);
            for (header, value) in &response.headers {
                eprintln!("< {}: {}", header, MaybeStr(value.as_bytes()));
            }
        }

        // By default we won't print binary to the terminal, but the user can
        // override this with `--output -`. We will happily write binary to a
        // file though
        let (mut output, allow_binary) = if let Some(path) = &self.output {
            let output: Box<dyn Write> = if path == Path::new("-") {
                Box::new(io::stdout())
            } else {
                Box::new(
                    OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open(path)
                        .with_context(|| {
                            format!("Error opening file `{}`", path.display())
                        })?,
                )
            };
            // The user explicitly asked for stdout, so we will write binary
            // here. This matches curl behavior
            (output as Box<dyn Write>, true)
        } else {
            let stdout = io::stdout();
            let allow_binary = !stdout.is_terminal();
            (Box::new(stdout) as Box<dyn Write>, allow_binary)
        };

        if response.body.text().is_none() && !allow_binary {
            eprintln!(
                "Response body is not text. Binary output can mess up your \
                terminal. Pass `--output -` if you're sure you want to print \
                the output, or consider `--output <FILE>` to save to a file."
            );
        } else {
            output.write_all(response.body.bytes())?;
        }
        Ok(())
    }
}

/// [HttpProvider] for the CLI. This will _not_ perform any persistence; that
/// should be handled by the request command implementation as needed.
#[derive(Debug)]
struct CliHttpProvider {
    database: CollectionDatabase,
    http_engine: HttpEngine,
    trigger_dependencies: bool,
}

#[async_trait]
impl HttpProvider for CliHttpProvider {
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, StoredRequestError> {
        self.database
            .get_latest_request(profile_id.into(), recipe_id)
            .map_err(StoredRequestError::new)
    }

    async fn send_request(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError> {
        if self.trigger_dependencies {
            let ticket = self.http_engine.build(seed, template_context).await?;
            let exchange = ticket.send().await?;
            Ok(exchange)
        } else {
            Err(TriggeredRequestError::NotAllowed)
        }
    }
}

/// Prompt the user for input on the CLI
#[derive(Debug)]
struct CliPrompter;

impl Prompter for CliPrompter {
    fn prompt(&self, prompt: Prompt) {
        // This will implicitly queue the prompts by blocking the main thread.
        // Since the CLI has nothing else to do while waiting on a response,
        // that's fine.
        let result = if prompt.sensitive {
            // Dialoguer doesn't support default values here so there's nothing
            // we can do
            if prompt.default.is_some() {
                warn!(
                    "Default value not supported for sensitive prompts in CLI"
                );
            }

            Password::new()
                .with_prompt(prompt.message)
                .allow_empty_password(true)
                .interact()
        } else {
            let mut input =
                Input::new().with_prompt(prompt.message).allow_empty(true);
            if let Some(default) = prompt.default {
                input = input.default(default);
            }
            input.interact()
        }
        .context("Error reading value from prompt")
        .traced();

        // If we failed to read the value, print an error and report nothing
        if let Ok(value) = result {
            prompt.channel.respond(value);
        }
    }

    fn select(&self, mut select: Select) {
        let result = DialoguerSelect::new()
            .with_prompt(select.message)
            .items(&select.options)
            .default(0)
            .interact();

        // If we failed to read the value, print an error and report nothing
        if let Ok(value) =
            result.context("Error reading value from select").traced()
        {
            select.channel.respond(select.options.swap_remove(value));
        }
    }
}

/// Parse a single key=value pair for an argument
fn parse_key_val<T, U>(
    s: &str,
) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid key=value: no \"=\" found in `{s}`"))?;
    Ok((key.parse()?, value.parse()?))
}
