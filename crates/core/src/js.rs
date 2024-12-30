use crate::{
    collection::{Collection, FunctionId},
    template::{Template, TemplateContext},
};
use anyhow::Context;
use rustyscript::{js_value::Function, Module, Runtime};
use std::{collections::HashMap, path::Path};
use tokio::sync::Mutex;
use tracing::{debug, info, info_span};

/// TODO
#[derive(derive_more::Debug)]
pub struct JsRuntime {
    #[debug(skip)]
    runtime: Mutex<Runtime>,
    functions: HashMap<FunctionId, Function>,
}

impl JsRuntime {
    /// TODO
    pub fn new() -> Self {
        // This function is independent from app state or user input, so an
        // error is very exceptional. It also means we probably can't do
        // anything meaningful, so it's alright to panic.

        let _ = info_span!("Initializing JS runtime").entered();
        let runtime = Runtime::new(Default::default()).unwrap();
        // TODO enable sandboxing?

        Self {
            runtime: runtime.into(),
            functions: HashMap::new(),
        }
    }

    /// Load a recipe collection from a JS file
    pub async fn load_collection(
        &mut self,
        path: &Path,
    ) -> anyhow::Result<Collection> {
        info!(?path, "Loading collection file");
        async {
            let module = Module::load(path)?;
            // Exported value should be a unary function that returns the
            // collection
            let mut runtime = self.runtime.lock().await;
            let handle = runtime.load_module_async(&module).await?;
            // TODO replace functions with func IDs somehow
            let collection: Collection =
                runtime.call_entrypoint_async(&handle, &()).await?;
            Ok::<_, anyhow::Error>(collection)
        }
        .await
        .with_context(|| format!("Error loading collection from {path:?}"))
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Renderer {
    /// TODO
    async fn render_bytes(
        &self,
        template: &Template,
    ) -> anyhow::Result<Vec<u8>> {
        match template {
            Template::Value(s) => Ok(s.clone().into_bytes()),
            Template::Lazy(function_id) => self
                .render_function(function_id)
                .await
                .map(String::into_bytes),
        }
    }

    /// TODO
    async fn render_string(
        &self,
        template: &Template,
    ) -> anyhow::Result<String> {
        match template {
            Template::Value(s) => Ok(s.clone()),
            Template::Lazy(function_id) => {
                self.render_function(function_id).await
            }
        }
    }

    /// TODO return bytes instead
    async fn render_function(
        &self,
        function_id: &FunctionId,
    ) -> anyhow::Result<String>;

    /// TODO
    fn context(&self) -> &TemplateContext;
}

pub struct PlainRenderer<'a> {
    pub runtime: &'a JsRuntime,
    pub context: &'a TemplateContext,
}

impl<'a> Renderer for PlainRenderer<'a> {
    /// TODO return bytes instead
    async fn render_function(
        &self,
        function_id: &FunctionId,
    ) -> anyhow::Result<String> {
        let _ = self.runtime.functions.get(function_id);
        todo!()
    }

    fn context(&self) -> &TemplateContext {
        self.context
    }
}
