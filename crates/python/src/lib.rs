use async_trait::async_trait;
use dialoguer::{Input, Password, Select as DialoguerSelect};
use indexmap::IndexMap;
use pyo3::{
    PyErr, PyResult,
    exceptions::{PyRuntimeError, PyValueError},
    pyclass, pymethods, pymodule,
};
use slumber_config::Config;
use slumber_core::{
    collection::{CollectionFile, ProfileId, RecipeId},
    database::{CollectionDatabase, Database},
    http::{
        BuildOptions, Exchange, HttpEngine, RequestRecord, RequestSeed,
        ResponseRecord, StoredRequestError, TriggeredRequestError,
    },
    render::{HttpProvider, Prompt, Prompter, SelectOption, TemplateContext},
};
use slumber_template::Template;
use std::{
    error::Error,
    fmt::{self, Display},
    path::PathBuf,
    str::Utf8Error,
    sync::{Arc, LazyLock},
};
use tokio::{
    runtime::{self, Runtime},
    sync::oneshot,
    task::LocalSet,
};

/// reqwest specifically needs a tokio runtime, so we need to spawn this in
/// addition to the python asyncio runtime. We'll spawn tasks on this rt and
/// await the task handle in the python event loop. This has to be a
/// multi-thread runtime because a current-thread deadlocks with python.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1)
        .build()
        .unwrap()
});

/// Python bindings for Slumber, the source-based REST/HTTP client.
/// [Documentation](https://slumber.lucaspickering.me/integration/python.html)
#[pymodule]
mod slumber {
    #[pymodule_export]
    use super::{Collection, Response};
}

/// A Slumber request collection
///
/// A request collection is the entrypoint for making requests in Slumber. It's
/// defined in a YAML file (typically called `slumber.yml`).
///
///[See docs for more information on how to build a request collection](
/// https://slumber.lucaspickering.me/api/request_collection/index.html)
///
/// ```python
/// import asyncio
/// from slumber import Collection
///
/// collection = Collection()
/// response = asyncio.run(collection.request('get_current_user'))
/// ```
#[pyclass]
#[expect(clippy::struct_field_names)]
struct Collection {
    tokio_handle: runtime::Handle,
    collection_file: CollectionFile,
    collection: Arc<slumber_core::collection::Collection>,
    database: CollectionDatabase,
    http_engine: HttpEngine,
}

#[pymethods]
impl Collection {
    /// Load a request collection
    ///
    /// By default, the collection file is selected from the current directory
    /// [according to these rules](https://slumber.lucaspickering.me/api/request_collection/index.html#format--loading).
    ///
    /// :param path: Load a specific collection file. If a directory is given,
    ///   load a file from that directory using the rules linked above.
    #[new]
    #[pyo3(signature = (path=None))]
    fn new(path: Option<PathBuf>) -> PyResult<Self> {
        let config = Config::load().map_err(ErrorDisplay::new)?;
        let collection_file =
            CollectionFile::new(path).map_err(ErrorDisplay::new)?;
        let collection = collection_file.load().map_err(ErrorDisplay::new)?;
        let database = Database::load()
            .and_then(|db| db.into_collection(&collection_file))
            .map_err(ErrorDisplay::new)?;
        let http_engine = HttpEngine::new(&config.http);

        Ok(Self {
            tokio_handle: RUNTIME.handle().clone(),
            collection_file,
            collection: Arc::new(collection),
            database,
            http_engine,
        })
    }

    /// Build and send an HTTP request for a recipe
    ///
    /// :param recipe: ID of the recipe
    /// :param profile: ID of the profile to use when building the request.
    ///   Defaults to the default profile in the collection, if any.
    /// :param overrides: Override individual profile fields with static values
    /// :param trigger: Trigger upstream requests? If disabled,
    ///   `response()`/`response_header()` calls in request templates will never
    ///   trigger request dependencies, meaning those requests must be run
    ///   manually.
    /// :return: The returned server response
    #[pyo3(signature = (
        recipe,
        profile=None,
        overrides=IndexMap::new(),
        trigger=true,
    ))]
    async fn request(
        &self,
        recipe: String,
        profile: Option<String>,
        overrides: IndexMap<String, String>,
        trigger: bool,
    ) -> PyResult<Response> {
        // reqwest/hyper need to be run in tokio, so we have to spawn this in
        // a background task instead of executing it in the python event loop.
        // Most of the context is !Send so we have to pin it to a single thread.
        // We can't use tokio::spawn because those futures require Send for
        // work-stealing.
        // https://docs.rs/tokio/1.49.0/tokio/task/struct.LocalSet.html#use-inside-tokiospawn
        let (tx, rx) = oneshot::channel();
        let rt = self.tokio_handle.clone();
        let request = Request {
            recipe_id: RecipeId::from(recipe),
            profile_id: profile.map(ProfileId::from),
            overrides,
            trigger,
            collection: Arc::clone(&self.collection),
            database: self.database.clone(),
            http_engine: self.http_engine.clone(),
            root_dir: self.collection_file.parent().to_owned(),
        };
        std::thread::spawn(move || {
            let local = LocalSet::new();
            local.spawn_local(async move {
                let result = request.send().await;
                tx.send(result).unwrap();
            });
            rt.block_on(local);
        });

        // We can await this in python's async engine because it's Send
        let exchange = rx.await.map_err(ErrorDisplay::new)??;

        // This is safe because no one else has the request/response
        let Ok(request) = Arc::try_unwrap(exchange.request) else {
            unreachable!("Request body was shared")
        };
        let Ok(response) = Arc::try_unwrap(exchange.response) else {
            unreachable!("Response body was shared")
        };
        Ok(Response { request, response })
    }

    /// Reload the collection from its file. Use this if you've made changes to
    /// the YAML file during a Python session, and want those changes to be
    /// reflected in Python.
    fn reload(&mut self) -> PyResult<()> {
        let collection =
            self.collection_file.load().map_err(ErrorDisplay::new)?;
        self.collection = Arc::new(collection);
        Ok(())
    }
}

/// HTTP response data
#[pyclass]
struct Response {
    request: RequestRecord,
    response: ResponseRecord,
}

#[pymethods]
impl Response {
    /// HTTP request URL
    #[getter]
    fn url(&self) -> &str {
        self.request.url.as_str()
    }

    /// HTTP spec version used
    #[getter]
    fn http_version(&self) -> &str {
        self.request.http_version.to_str()
    }

    /// HTTP status code of the response
    #[getter]
    fn status_code(&self) -> u16 {
        self.response.status.as_u16()
    }

    /// HTTP headers of the response
    #[getter]
    fn headers(&self) -> PyResult<IndexMap<String, String>> {
        self.response
            .headers
            .iter()
            .map(|(name, value)| {
                let value = value.to_str().map_err(ErrorDisplay::new)?;
                Ok((name.to_string(), value.to_owned()))
            })
            .collect()
    }

    /// Response content bytes
    #[getter]
    fn content(&self) -> &[u8] {
        self.response.body.bytes()
    }

    /// Response content decoded as UTF-8
    #[getter]
    fn text(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(self.response.body.bytes())
    }

    /// If the response status code is >= 400, raise an exception
    fn raise_for_status(&self) -> PyResult<()> {
        let status = self.response.status;
        if status.as_u16() < 400 {
            Ok(())
        } else {
            Err(PyValueError::new_err(format!("Status code {status}")))
        }
    }
}

#[derive(Clone, Debug)]
struct PythonHttpProvider {
    database: CollectionDatabase,
    http_engine: HttpEngine,
    trigger_dependencies: bool,
}

#[async_trait(?Send)]
impl HttpProvider for PythonHttpProvider {
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

#[derive(Debug)]
struct PythonPrompter;

impl PythonPrompter {
    /// Ask the user for text input
    fn text(
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> Result<String, dialoguer::Error> {
        // This will implicitly queue the prompts by blocking the only worker
        // thread. Since the library has nothing to do while waiting on a
        // response, that's fine
        if sensitive {
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
    }

    /// Ask the user to select a value from a list. Return the selected value.
    fn select(
        message: String,
        mut options: Vec<SelectOption>,
    ) -> Result<slumber_template::Value, dialoguer::Error> {
        let index = DialoguerSelect::new()
            .with_prompt(message)
            .items(&options)
            .default(0)
            .interact()?;

        Ok(options.swap_remove(index).value)
    }
}

impl Prompter for PythonPrompter {
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

/// Wrapper to stringify an error and convert it to Python. This is clumsy
/// because it converts everything to a `RuntimeError`, but it's simple and
/// effective.
struct ErrorDisplay(Box<dyn 'static + Error + Send + Sync>);

impl ErrorDisplay {
    fn new(error: impl 'static + Error + Send + Sync) -> Self {
        Self(Box::new(error))
    }
}

impl Display for ErrorDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Write the entire error chain
        let error = &self.0;
        write!(f, "{error}")?;
        let mut source = error.source();
        while let Some(error) = source {
            write!(f, ": {error}")?;
            source = error.source();
        }
        Ok(())
    }
}

impl From<ErrorDisplay> for PyErr {
    fn from(value: ErrorDisplay) -> Self {
        PyRuntimeError::new_err(value.to_string())
    }
}

struct Request {
    recipe_id: RecipeId,
    profile_id: Option<ProfileId>,
    overrides: IndexMap<String, String>,
    trigger: bool,
    collection: Arc<slumber_core::collection::Collection>,
    database: CollectionDatabase,
    http_engine: HttpEngine,
    root_dir: PathBuf,
}

impl Request {
    /// Send a request and return the request+response
    async fn send(self) -> PyResult<Exchange> {
        let selected_profile = self.profile_id.or_else(|| {
            // Use the default profile if none is given
            self.collection
                .default_profile()
                .map(|profile| profile.id.clone())
        });

        // reqwest/hyper need to be run in tokio, so we have to spawn this in
        // a background task instead of executing it in the python event loop.
        // Most of the context is !Send so we have to move it all in.
        let http_provider = PythonHttpProvider {
            database: self.database,
            http_engine: self.http_engine.clone(),
            trigger_dependencies: self.trigger,
        };
        let overrides = self
            .overrides
            .into_iter()
            // Don't support templates in overrides (yet)
            .map(|(field, value)| (field, Template::raw(value)))
            .collect();
        let context = TemplateContext {
            collection: self.collection,
            selected_profile,
            http_provider: Box::new(http_provider),
            overrides,
            prompter: Box::new(PythonPrompter),
            show_sensitive: true,
            root_dir: self.root_dir,
            state: Default::default(),
        };
        let seed = RequestSeed::new(self.recipe_id, BuildOptions::default());

        let ticket = self
            .http_engine
            .build(seed, &context)
            .await
            .map_err(ErrorDisplay::new)?;
        let exchange = ticket.send().await.map_err(ErrorDisplay::new)?;
        Ok(exchange)
    }
}
