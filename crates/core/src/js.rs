use crate::{
    collection::{
        Authentication, Collection, Folder, FunctionId, Profile, Recipe,
        RecipeBody, RecipeNode, RecipeTree,
    },
    template::{Template, TemplateContext},
};
use anyhow::Context as _;
use indexmap::IndexMap;
use rquickjs::{AsyncContext, AsyncRuntime, Function, Module};
use std::{collections::HashMap, hash::Hash, path::Path};
use tokio::fs;
use tracing::{debug, info, info_span};

/// TODO
/// TODO rename
#[derive(derive_more::Debug)]
pub struct JsVm {
    #[debug(skip)]
    runtime: AsyncRuntime,
    #[debug(skip)]
    context: AsyncContext,
}

impl JsVm {
    /// TODO
    pub async fn new() -> Self {
        // This function is independent from app state or user input, so an
        // error is very exceptional. It also means we probably can't do
        // anything meaningful, so it's alright to panic.

        let _ = info_span!("Initializing JS runtime").entered();
        let runtime = AsyncRuntime::new().unwrap();
        // TODO should we use a more limited context?
        let context = AsyncContext::full(&runtime).await.unwrap();

        Self { runtime, context }
    }

    /// Load a recipe collection from a JS file
    pub async fn load_collection(
        &self,
        path: &Path,
    ) -> anyhow::Result<Collection> {
        info!(?path, "Loading collection file");
        let source = fs::read_to_string(path).await.context("TODO")?;
        self.context
            .with(|context| {
                let func =
                    context.eval_file::<Function, _>(path).context("asdf")?;
                // let func = Module::evaluate(context.clone(), "test", source)?
                //     .finish::<Function>()?;

                // Deserialize with real function pointers
                let collection = func.call::<_, Collection<Function>>(())?;

                // Replace the function pointers with unique IDs. This allows
                // the collection to impl Send. During rendering
                // we'll use the map to convert IDs back to
                // functions TODO where to store the functions?
                let mut functions = FunctionRegistry::default();
                Ok::<_, anyhow::Error>(collection.convert(&mut functions))
            })
            .await
            .with_context(|| format!("Error loading collection from {path:?}"))
    }
}
/*
/// A helper for rendering templates. This handles the JS side of rendering,
/// i.e. calling deferred render functions.
#[derive(derive_more::Debug)]
pub struct JsRenderer {
    #[debug(skip)]
    js: AsyncContext,
    /// Template context, to provide values and utilities to Slumber functions
    /// during template renders. We hold a reference here, as well as one in
    /// the JS runtime, so that Rust-in-JS functions can use it.
    template_context: Arc<TemplateContext>,
}

impl JsRenderer {
    /// TODO
    pub fn context(&self) -> &TemplateContext {
        &self.template_context
    }

    /// TODO
    pub async fn render_bytes(
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
    pub async fn render_string(
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

    async fn render_function(&self, _: &FunctionId) -> anyhow::Result<String> {
        todo!()
    }
} */

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
    js_context: &'a AsyncContext,
    template_context: &'a TemplateContext,
}

impl<'a> PlainRenderer<'a> {
    pub fn new(js: &'a JsVm, template_context: &'a TemplateContext) -> Self {
        Self {
            js_context: &js.context,
            template_context,
        }
    }
}

impl<'a> Renderer for PlainRenderer<'a> {
    /// TODO return bytes instead
    async fn render_function(
        &self,
        function_id: &FunctionId,
    ) -> anyhow::Result<String> {
        let empty = IndexMap::new();
        // TODO render profile fields first
        let profile_data = self
            .template_context
            .profile()
            .map(|profile| &profile.data)
            .unwrap_or(&empty);
        todo!()
        // func.call_async(&mut runtime, None, &[profile_data])
        //     .await
        //     .context("TODO")
    }

    fn context(&self) -> &TemplateContext {
        self.template_context
    }
}

type FunctionRegistry = HashMap<FunctionId, ()>;

trait ConvertFns {
    type Output;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output;
}

impl<'js> ConvertFns for Collection<Function<'js>> {
    type Output = Collection<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        Collection {
            profiles: self.profiles.convert(registry),
            recipes: self.recipes.convert(registry),
        }
    }
}

impl<K: Eq + Hash + PartialEq, V: ConvertFns> ConvertFns for IndexMap<K, V> {
    type Output = IndexMap<K, V::Output>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        self.into_iter()
            .map(|(k, v)| (k, v.convert(registry)))
            .collect()
    }
}

impl<'js> ConvertFns for Profile<Function<'js>> {
    type Output = Profile<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        Profile {
            id: self.id,
            name: self.name,
            default: self.default,
            data: self.data.convert(registry),
        }
    }
}

impl<'js> ConvertFns for RecipeTree<Function<'js>> {
    type Output = RecipeTree<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        RecipeTree {
            tree: self.tree.convert(registry),
            nodes_by_id: self.nodes_by_id,
        }
    }
}

impl<'js> ConvertFns for RecipeNode<Function<'js>> {
    type Output = RecipeNode<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        match self {
            Self::Folder(folder) => {
                RecipeNode::Folder(folder.convert(registry))
            }
            Self::Recipe(recipe) => {
                RecipeNode::Recipe(recipe.convert(registry))
            }
        }
    }
}

impl<'js> ConvertFns for Recipe<Function<'js>> {
    type Output = Recipe<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        Recipe {
            id: self.id,
            name: self.name,
            method: self.method,
            url: self.url.convert(registry),
            body: self.body.map(|body| body.convert(registry)),
            authentication: self
                .authentication
                .map(|authentication| authentication.convert(registry)),
            query: self
                .query
                .into_iter()
                .map(|(param, value)| (param, value.convert(registry)))
                .collect(),
            headers: self.headers.convert(registry),
        }
    }
}

impl<'js> ConvertFns for RecipeBody<Function<'js>> {
    type Output = RecipeBody<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        match self {
            Self::Raw { body, content_type } => RecipeBody::Raw {
                body: body.convert(registry),
                content_type,
            },
            Self::FormUrlencoded(fields) => {
                RecipeBody::FormUrlencoded(fields.convert(registry))
            }
            Self::FormMultipart(fields) => {
                RecipeBody::FormMultipart(fields.convert(registry))
            }
        }
    }
}

impl<'js> ConvertFns for Authentication<Template<Function<'js>>> {
    type Output = Authentication<Template<FunctionId>>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        match self {
            Self::Basic { username, password } => Authentication::Basic {
                username: username.convert(registry),
                password: password.map(|password| password.convert(registry)),
            },
            Self::Bearer(token) => {
                Authentication::Bearer(token.convert(registry))
            }
        }
    }
}

impl<'js> ConvertFns for Folder<Function<'js>> {
    type Output = Folder<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        Folder {
            id: self.id,
            name: self.name,
            children: self.children.convert(registry),
        }
    }
}

impl<'js> ConvertFns for Template<Function<'js>> {
    type Output = Template<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        match self {
            Self::Value(s) => Template::Value(s),
            Self::Lazy(_) => {
                let id = FunctionId::new();
                // TODO store the actual fn
                registry.insert(id, ());
                Template::Lazy(id)
            }
        }
    }
}
