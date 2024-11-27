mod error;
mod functions;

pub use error::{FunctionError, LuaError};
pub use functions::{
    ChooseFn, CommandFn, EnvFn, FileFn, JsonPathFn, LuaFunction, ProfileFn,
    PromptFn, ResponseArgs, ResponseHeaderArgs,
};

use crate::{
    collection::{Collection, RecipeBody, RecipeId, RequestTrigger},
    http::{Exchange, RequestSeed, ResponseRecord},
    template::{TemplateContext, TriggeredRequestError},
    test_util::test_data_dir,
};
use chrono::Utc;
use futures::future;
use mlua::{ErrorContext, FromLua, IntoLua, Lua, LuaSerdeExt, Table, Value};
use std::{path::Path, sync::Arc};
use tracing::{debug, info, info_span, trace};

// There are lots of places in here where we call fallible functions on the Lua
// VM, but there's no known scenario where they can actually fail. These all
// use unwraps, because an error would be unrecoverable and probably fatal to
// program operation.

/// Field in the global scope holding the collection table
const COLLECTION_FIELD: &str = "collection";

/// Newtype wrapper for a Lua VM. This ensures that all Lua VMs are initialized
/// with sandbox settings, global functions, and anything else that _all_ VMs
/// should have. A VM can be used for two purposes:
/// - Loading a collection
/// - Rendering templates with [renderer](Self::renderer)
///
/// The goal of this struct is that the [mlua::Lua] type is never used outside
/// this module. All Lua actions should be routed through this struct, to make
/// sure the VM is always configured correctly.
#[derive(Clone, derive_more::Debug)]
pub struct LuaVm {
    #[debug(skip)]
    lua: Lua,
}

impl LuaVm {
    /// Initialize a new Lua VM. This is not _too_ expensive, so we can do it
    /// many times throughout a session, but shouldn't do it e.g. every render
    /// loop.
    pub fn new() -> Self {
        // This function is independent from app state or user input, so an
        // error is very exceptional. It also means we probably can't do
        // anything meaningful, so it's alright to panic.

        let _ = info_span!("Initializing Lua VM").entered();
        let lua = Lua::new();
        lua.sandbox(true).unwrap();

        // All Slumber functions will be grouped into a module. These are only
        // usable in a template context, but we register them for all VMs so we
        // can provide a useful error msg if a user calls it elsewhere
        Self::register_fn::<ChooseFn>(&lua);
        Self::register_fn::<CommandFn>(&lua);
        Self::register_fn::<EnvFn>(&lua);
        Self::register_fn::<FileFn>(&lua);
        Self::register_fn::<JsonPathFn>(&lua);
        Self::register_fn::<ProfileFn>(&lua);
        Self::register_fn::<PromptFn>(&lua);
        Self::register_fn::<ResponseArgs>(&lua);
        Self::register_fn::<ResponseHeaderArgs>(&lua);

        Self { lua }
    }

    /// Load a recipe collection from a Lua file
    pub fn load_collection(&self, path: &Path) -> Result<Collection, LuaError> {
        info!(?path, "Loading collection file");
        let load = || -> mlua::Result<Collection> {
            self.lua.load(path).exec()?;
            let value: Value = self.lua.globals().get(COLLECTION_FIELD)?;
            self.lua.from_value::<Collection>(value)
        };
        let collection = load().with_context(|_| {
            format!("Error loading collection from {path:?}")
        })?;
        Ok(collection)
    }

    /// Convert this VM to be usable for template rendering. Every set of
    /// renders (e.g. every rendered recipe) should use a clean VM, for
    /// isolation from other renders. To initialize the VM we'll have to
    /// execute the initial collection file.
    pub fn renderer(
        self,
        collection_path: &Path,
        context: TemplateContext,
    ) -> Result<LuaRenderer, LuaError> {
        let context = Arc::new(context);

        // Execute the collection file to initialize the global scope, e.g.
        // user-defined functions. We don't need to actually load the collection
        // object into a Rust type though, since we won't use it.
        self.lua
            .load(collection_path)
            // Error case is _probably_ unreachable because we can't get this
            // far into a program without loading the collection during startup
            .exec()?;
        self.lua.set_app_data(Arc::clone(&context));

        Ok(LuaRenderer::new(self, context))
    }

    /// Register a Slumber-provided function in the given VM
    fn register_fn<F: LuaFunction>(lua: &Lua) {
        let lua_fn = lua
            .create_async_function(|lua: Lua, args: mlua::Value| async move {
                let context = {
                    // This reference grab will never fail because we never
                    // grab a _mutable_ ref to this app data anywhere
                    let context = lua
                        .app_data_ref::<Arc<TemplateContext>>()
                        .ok_or_else(|| {
                            mlua::Error::runtime(
                                "Template context not available; Slumber \
                                functions can only be used within a template",
                            )
                        })?;
                    Arc::clone(&*context)
                };
                let args: F =
                    lua.from_value(args).context("Invalid arguments")?;
                // Constructing a new VM wrapper here is safe, because we know
                // the VM here was created safely originally
                let renderer = LuaRenderer::new(LuaVm { lua }, context);
                let bytes = args.call(renderer).await?;
                Ok(bytes)
            })
            .unwrap();
        lua.globals().set(F::NAME, lua_fn).unwrap();
    }
}

impl Default for LuaVm {
    fn default() -> Self {
        Self::new()
    }
}

/// A helper for rendering templates. This handles the Lua side of rendering,
/// i.e. evaluating expressions within each template key.
#[derive(Debug)]
pub struct LuaRenderer {
    lua: LuaVm,
    /// Template context, to provide values and utilities to Slumber functions
    /// during template renders. We hold a reference here, as well as one in
    /// the Lua VM (via "app data"), so that Rust-in-Lua functions can use it.
    context: Arc<TemplateContext>,
}

impl LuaRenderer {
    fn new(lua: LuaVm, context: Arc<TemplateContext>) -> Self {
        Self { lua, context }
    }

    pub fn context(&self) -> &TemplateContext {
        &self.context
    }

    /// Evaluate a Lua expression, returning bytes that most often represent
    /// a Rust string
    pub async fn eval(&self, source: &str) -> Result<Vec<u8>, LuaError> {
        trace!(source, "Evaluting Lua expression");
        let output: LuaWrap<Vec<u8>> =
            self.lua.lua.load(source).eval_async().await.with_context(
                |_| format!("Error in Lua expression `{source}`"),
            )?;
        Ok(output.0)
    }

    /// Render the select profile to a Lua table. Each profile value will be
    /// rendered by a template. Profile data will *not* be available during
    /// these renders, because it's not yet available!
    async fn render_profile(&self) -> Result<Table, FunctionError> {
        let lua = &self.lua.lua;
        let table = lua.create_table().unwrap();

        let Some(profile) = self.context().profile() else {
            return Ok(table);
        };
        debug!(profile_id = %profile.id, "Rendering profile");
        let rendered = future::try_join_all(profile.data.iter().map(
            |(key, template)| async move {
                let rendered =
                    template.render(self).await.map_err(|error| {
                        FunctionError::Template {
                            error,
                            template: template.clone(),
                        }
                    })?;
                Ok::<_, FunctionError>((key, LuaWrap(rendered)))
            },
        ))
        .await?;

        for (k, v) in rendered {
            table.set(k.as_str(), v).unwrap();
        }

        Ok(table)
    }

    /// Get the most recent response for a profile+recipe pair
    async fn get_latest_response(
        &self,
        recipe_id: &RecipeId,
        trigger: RequestTrigger,
    ) -> Result<ResponseRecord, FunctionError> {
        let context = self.context();

        // Defer loading the most recent exchange until we know we'll need it
        let get_latest = || -> Result<Option<Exchange>, FunctionError> {
            context
                .database
                .get_latest_request(
                    context.selected_profile.as_ref(),
                    recipe_id,
                )
                .map_err(FunctionError::Database)
        };

        // Helper to execute the request, if triggered
        let send_request = || async {
            // There are 3 different ways we can generate the request config:
            // 1. Default (enable all query params/headers)
            // 2. Load from UI state for both TUI and CLI
            // 3. Load from UI state for TUI, enable all for CLI
            // These all have their own issues:
            // 1. Triggered request doesn't necessarily match behavior if user
            //  were to execute the request themself
            // 2. CLI behavior is silently controlled by UI state
            // 3. TUI and CLI behavior may not match
            // All 3 options are unintuitive in some way, but 1 is the easiest
            // to implement so I'm going with that for now.
            let build_options = Default::default();

            // Shitty try block
            async {
                let http_engine = context
                    .http_engine
                    .as_ref()
                    .ok_or(TriggeredRequestError::NotAllowed)?;
                let ticket = http_engine
                    .build(
                        RequestSeed::new(recipe_id.clone(), build_options),
                        self,
                    )
                    .await
                    .map_err(|error| {
                        TriggeredRequestError::Build(error.into())
                    })?;
                ticket
                    .send(&context.database)
                    .await
                    .map_err(|error| TriggeredRequestError::Send(error.into()))
            }
            .await
            .map_err(|error| FunctionError::Trigger {
                recipe_id: recipe_id.clone(),
                error,
            })
        };

        let exchange = match trigger {
            RequestTrigger::Never => {
                get_latest()?.ok_or(FunctionError::ResponseMissing)?
            }
            RequestTrigger::NoHistory => {
                // If a exchange is present in history, use that. If not, fetch
                if let Some(exchange) = get_latest()? {
                    exchange
                } else {
                    send_request().await?
                }
            }
            RequestTrigger::Expire { duration } => match get_latest()? {
                Some(exchange)
                    if exchange.end_time + duration >= Utc::now() =>
                {
                    exchange
                }
                _ => send_request().await?,
            },
            RequestTrigger::Always => send_request().await?,
        };

        Ok(exchange.response)
    }
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for LuaRenderer {
    fn factory(_: ()) -> Self {
        Self::factory(TemplateContext::factory(()))
    }
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory<TemplateContext> for LuaRenderer {
    fn factory(context: TemplateContext) -> Self {
        LuaVm::new()
            .renderer(&test_data_dir().join("empty.luau"), context)
            .unwrap()
    }
}

/// Wrapper for values to be converted to/from Lua types. This is just to get
/// around the orphan rule.
pub struct LuaWrap<T>(T);

impl<T> From<T> for LuaWrap<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: AsRef<[u8]>> IntoLua for LuaWrap<T> {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<Value> {
        // Lua strings are arbitrary bytes, so we can stuff anything in there
        lua.create_string(&self.0).map(Value::String)
    }
}

impl FromLua for LuaWrap<Vec<u8>> {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match value {
            // Clones here are unavoidable (I think) because we have to copy
            // from the VM's memory into our own. In most cases these bytes are
            // coming from Rust originally so it sucks that we have to copy into
            // the VM and back out.
            Value::String(string) => Ok(Self(string.as_bytes().to_owned())),
            Value::Buffer(buffer) => Ok(Self(buffer.to_vec())),
            _ => Err(mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: "bytes".to_owned(),
                message: Some("Expected string or buffer".to_owned()),
            }),
        }
    }
}

impl IntoLua for RecipeBody {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        lua.to_value(&self)
    }
}
