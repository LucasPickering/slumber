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
    collection::{
        Authentication, ProfileId, QueryParameterValue, Recipe, RecipeId,
    },
    database::{CollectionDatabase, Database},
    http::{
        BuildFieldOverride, BuildOptions, Exchange, HttpEngine, RequestRecord,
        RequestSeed, ResponseRecord, StoredRequestError, TriggeredRequestError,
    },
    render::{HttpProvider, Prompt, Prompter, SelectOption, TemplateContext},
    util::MaybeStr,
};
use slumber_template::{Expression, Template};
use slumber_util::{ResultTraced, ResultTracedAnyhow};
use std::{
    error::Error,
    fs::OpenOptions,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use tracing::warn;

/// Exit code to return when `exit_status` flag is set and the HTTP response has
/// an error status code
const HTTP_ERROR_EXIT_CODE: u8 = 2;

/// Execute a single request and print its response
#[derive(Clone, Debug, Parser)]
#[clap(visible_aliases = &["req", "rq"])]
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

    /// Set credentials for HTTP Basic authentication
    ///
    /// The username and password are split on the first colon. This means the
    /// username cannot contain a colon. If the colon is omitted, you will be
    /// prompted for a password instead. Both username and password will be
    /// parsed and rendered as templates. The split on colon occurs before the
    /// template parsing.
    ///
    /// The request will use basic authentication whether the recipe is
    /// configured for it or not.
    // Alias for curl compatibility
    #[clap(
        long,
        visible_alias = "user",
        conflicts_with = "bearer",
        value_hint = ValueHint::Other,
        value_name = "username:password",
    )]
    basic: Option<String>,

    /// Set token for HTTP Bearer authentication
    ///
    /// The token is parsed and rendered as a template.
    ///
    /// The request will use bearer authentication whether the recipe is
    /// configured for it or not.
    #[clap(
        long,
        visible_alias = "token",
        value_hint = ValueHint::Other,
        value_name = "token",
    )]
    bearer: Option<Template>,

    /// Override the request body
    ///
    /// The behavior of this override is dependent on the body's original type
    /// in the recipe:
    /// - If there is no body, the given override will become a raw body
    /// - Raw and stream bodies are replaced directly
    /// - JSON bodies are parsed as JSON before being rendered as a string
    /// - Form bodies CANNOT be overridden by this flag
    #[clap(long, visible_alias = "data", value_hint = ValueHint::Other)]
    body: Option<Template>,

    /// Override a request form field (format: `field=value`)
    ///
    /// The given value is parsed and rendered as a template. To override
    /// multiple headers, pass this flag multiple times. Requires the recipe
    /// to have a form_urlencoded or form_multipart body.
    ///
    ///   slumber request my-recipe -F 'my-field={{my_field}}'
    ///
    /// To omit the form field entirely, exclude the = and value:
    ///
    ///   slumber request my-recipe -F my-field
    #[clap(
        long,
        short = 'F',
        value_parser = parse_recipe_override,
        value_hint = ValueHint::Other, // Disable completions
        value_name = "field=value",
        verbatim_doc_comment,
    )]
    form: Vec<(String, BuildFieldOverride)>,

    /// Override a request header (format: `header=value`)
    ///
    /// The given value is parsed and rendered as a template. To override
    /// multiple headers, pass this flag multiple times.
    ///
    ///   slumber request my-recipe -H 'X-My-Header={{my_header}}'
    ///
    /// To omit the header entirely, exclude the = and value:
    ///
    ///   slumber request my-recipe -H X-My-Header
    #[clap(
        long,
        short = 'H',
        value_parser = parse_recipe_override,
        value_hint = ValueHint::Other, // Disable completions
        value_name = "header=value",
        verbatim_doc_comment,
    )]
    header: Vec<(String, BuildFieldOverride)>,

    /// Override the value of a profile field (format: `field=value`)
    ///
    /// The given value is parsed as a template. To override multiple fields,
    /// pass this flag multiple times.
    ///
    ///   slumber request my-recipe -o foo=bar -o 'username={{username}}'
    #[clap(
        long = "override",
        short = 'o',
        value_parser = parse_profile_override,
        value_hint = ValueHint::Other, // Disable completions
        value_name = "field=value",
        verbatim_doc_comment,
    )]
    overrides: Vec<(String, Template)>,

    /// Override a request query parameter (format: `parameter=value`)
    ///
    /// The given value is parsed as a template. To override multiple
    /// parameters, or to specify the same parameter multiple times, pass
    /// this flag multiple times.
    ///
    ///   slumber request my-recipe --query foo=bar
    ///
    /// Any parameter that appears in an override will replace ALL instances of
    /// that parameter from the recipe definition. If you only want to override
    /// a single instance of the parameter, you'll need to re-specify all
    /// instances in the override flags.
    #[clap(
        long,
        value_parser = parse_recipe_override,
        value_hint = ValueHint::Other, // Disable completions
        value_name = "query=value",
        verbatim_doc_comment,
    )]
    query: Vec<(String, BuildFieldOverride)>,

    /// Set the URL for the request
    ///
    /// The URL is parsed and rendered as a template. This will override the
    /// `url` field of the recipe entirely. Query parameters from the recipe
    /// will *not* be replaced; they will be appended to the rendered URL
    /// override.
    #[clap(long, value_hint = ValueHint::Other)]
    url: Option<Template>,
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
    #[clap(long, value_name = "path")]
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
        database.set_name(&collection);
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
        let authentication = match (self.basic, self.bearer) {
            (None, None) => None,
            (None, Some(token)) => Some(Authentication::Bearer { token }),
            (Some(value), None) => Some(get_basic_auth(&value)?),
            (Some(_), Some(_)) => {
                // Mutual exclusivity is handled by clap
                unreachable!("--basic and --bearer are mutually exclusive")
            }
        };
        let recipe = collection.recipes.try_get_recipe(&self.recipe_id)?;
        let build_options = BuildOptions {
            url: self.url,
            authentication,
            headers: IndexMap::from_iter(self.header),
            body: self.body,
            query_parameters: get_query_parameters(recipe, self.query),
            form_fields: IndexMap::from_iter(self.form),
        };
        let template_context = TemplateContext {
            selected_profile,
            collection: collection.into(),
            http_provider: Box::new(CliHttpProvider {
                database: database.clone(),
                http_engine: http_engine.clone(),
                trigger_dependencies,
            }),
            overrides: IndexMap::from_iter(self.overrides),
            prompter: Box::new(CliPrompter),
            show_sensitive: true,
            root_dir: collection_file.parent().to_owned(),
            state: Default::default(),
        };
        let seed = RequestSeed::new(self.recipe_id, build_options);
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

#[async_trait(?Send)]
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

impl CliPrompter {
    /// Ask the user for text input
    fn text(
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> anyhow::Result<String> {
        // This will implicitly queue the prompts by blocking the main thread.
        // Since the CLI has nothing else to do while waiting on a response,
        // that's fine.
        if sensitive {
            // Dialoguer doesn't support default values here so there's nothing
            // we can do
            if default.is_some() {
                warn!(
                    "Default value not supported for sensitive prompts in CLI"
                );
            }

            Password::new()
                .with_prompt(message)
                .allow_empty_password(true)
                .interact()
        } else {
            let mut input = Input::new().with_prompt(message).allow_empty(true);
            if let Some(default) = default {
                input = input.default(default);
            }
            input.interact()
        }
        // If we failed to read the value, print an error and report nothing
        .context("Error reading value from prompt")
        .traced()
    }

    /// Ask the user to select a value from a list. Return the selected value.
    fn select(
        message: String,
        mut options: Vec<SelectOption>,
    ) -> anyhow::Result<slumber_template::Value> {
        let index = DialoguerSelect::new()
            .with_prompt(message)
            .items(&options)
            .default(0)
            .interact()
            // If we failed to read the value, print an error and report nothing
            .context("Error reading value from select")
            .traced()?;
        Ok(options.swap_remove(index).value)
    }
}

impl Prompter for CliPrompter {
    fn prompt(&self, prompt: Prompt) {
        match prompt {
            Prompt::Text {
                message,
                default,
                sensitive,
                channel,
            } => {
                if let Ok(response) = Self::text(message, default, sensitive) {
                    channel.reply(response);
                }
            }
            Prompt::Select {
                message,
                options,
                channel,
            } => {
                if let Ok(response) = Self::select(message, options) {
                    channel.reply(response);
                }
            }
        }
    }
}

/// Parse a single key=value pair for a profile override. The `=` must be
/// present. Profile fields cannot be omitted.
fn parse_profile_override(
    s: &str,
) -> Result<(String, Template), anyhow::Error> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid key=value: no \"=\" found in `{s}`"))?;
    Ok((key.to_owned(), value.parse()?))
}

/// Parse a single key=value pair for a recipe override argument. If the `=`
/// sign is not present, the field is omitted instead of being overridden.
fn parse_recipe_override(
    s: &str,
) -> Result<(String, BuildFieldOverride), Box<dyn Error + Send + Sync + 'static>>
{
    if let Some((key, value)) = s.split_once('=') {
        // = sign => provide an override value
        let template: Template = value.parse()?;
        Ok((key.to_owned(), BuildFieldOverride::Override(template)))
    } else {
        // No = sign => omit the field
        Ok((s.to_owned(), BuildFieldOverride::Omit))
    }
}

/// Split the value into `username:password`. Each component will be parsed as
/// a template. If there is no colon, the entire value is the username and the
/// password will be a template to prompt the user for it.
fn get_basic_auth(value: &str) -> anyhow::Result<Authentication> {
    let (username, password) =
        if let Some((username, password)) = value.split_once(':') {
            let username: Template =
                username.parse().context("Invalid username template")?;
            let password: Template =
                password.parse().context("Invalid password template")?;
            (username, password)
        } else {
            let username: Template =
                value.parse().context("Invalid username template")?;
            // Generate a template that will prompt for the password. This lets
            // us reuse the prompter machinery instead of defining a
            // bespoke prompt here. Plus, it's cool!
            let password = Expression::call(
                "prompt",
                [],
                [
                    ("message", Some("Password".into())),
                    ("sensitive", Some(true.into())),
                ],
            )
            .into();
            (username, password)
        };
    Ok(Authentication::Basic {
        username,
        password: Some(password),
    })
}

/// Apply overrides to the query parameters in the recipe, returning the set
/// of overrides that can be passed to [BuildOptions].
///
/// Any query parameter that appears in the override list will be completely
/// overridden, meaning *all* instances of that parameter from the recipe will
/// be overridden or omitted. We need the recipe as input so we know which
/// `(param, index)` pairs need to be omitted when not overridden.
fn get_query_parameters(
    recipe: &Recipe,
    overrides: Vec<(String, BuildFieldOverride)>,
) -> IndexMap<(String, usize), BuildFieldOverride> {
    // Get the number of values a param has in the recipe
    let get_n = |param: &str| -> usize {
        match recipe.query.get(param) {
            None => 0,
            Some(QueryParameterValue::One(_)) => 1,
            Some(QueryParameterValue::Many(values)) => values.len(),
        }
    };

    overrides
        .into_iter()
        // Sort and group by parameter
        .sorted_by(|(a, _), (b, _)| String::cmp(a, b))
        // Clone is needed to detach the lifetime from the iterator
        .chunk_by(|(param, _)| param.clone())
        .into_iter()
        // For each param, spit out a series of overrides for each associated
        // value. If the recipe specifies `n` values and we have `o` overrides,
        // the output will be `max(n, o)` elements.
        .flat_map(|(param, values)| {
            values
                // If `n > o`, pad out with omits for values `(o+1)..n`
                .pad_using(get_n(&param), move |_| {
                    (param.clone(), BuildFieldOverride::Omit)
                })
                .enumerate()
                .map(|(i, (param, value))| ((param, i), value))
        })
        .collect()
}
