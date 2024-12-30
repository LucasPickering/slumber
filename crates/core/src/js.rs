use crate::{
    collection::{
        Authentication, Collection, Folder, FunctionId, Profile, Recipe,
        RecipeBody, RecipeNode, RecipeTree,
    },
    template::{Template, TemplateContext},
};
use anyhow::Context;
use indexmap::IndexMap;
use rustyscript::{js_value::Function, Module};
use std::{collections::HashMap, hash::Hash, path::Path};
use tokio::{runtime::Handle, sync::Mutex};
use tracing::{debug, info, info_span};

/// TODO
#[derive(derive_more::Debug)]
pub struct JsRuntime {
    #[debug(skip)]
    runtime: Mutex<rustyscript::Runtime>,
    functions: FunctionRegistry,
}

impl JsRuntime {
    /// TODO
    pub fn new() -> Self {
        // This function is independent from app state or user input, so an
        // error is very exceptional. It also means we probably can't do
        // anything meaningful, so it's alright to panic.

        let _ = info_span!("Initializing JS runtime").entered();
        let runtime = rustyscript::Runtime::with_tokio_runtime(
            Default::default(),
            Handle::current(),
        )
        .unwrap();
        // TODO enable sandboxing?

        Self {
            runtime: runtime.into(),
            functions: FunctionRegistry::default(),
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
            // Deserialize with real function pointers
            let collection: Collection<Function> =
                runtime.call_entrypoint_async(&handle, &()).await?;

            // Replace the function pointers with unique IDs. This allows the
            // collection to impl Send. During rendering we'll use the map to
            // convert IDs back to functions
            self.functions.functions.clear();
            let collection = collection.convert(&mut self.functions);

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

#[derive(Debug, Default)]
struct FunctionRegistry {
    functions: HashMap<FunctionId, Function>,
}

impl FunctionRegistry {
    fn register(&mut self, function: Function) -> FunctionId {
        let id = FunctionId::new();
        self.functions.insert(id, function);
        id
    }

    fn get(&self, id: &FunctionId) -> &Function {
        self.functions.get(id).expect("TODO")
    }
}

trait ConvertFns {
    type Output;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output;
}

impl ConvertFns for Collection<Function> {
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

impl ConvertFns for Profile<Function> {
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

impl ConvertFns for RecipeTree<Function> {
    type Output = RecipeTree<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        RecipeTree {
            tree: self.tree.convert(registry),
            nodes_by_id: self.nodes_by_id,
        }
    }
}

impl ConvertFns for RecipeNode<Function> {
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

impl ConvertFns for Recipe<Function> {
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

impl ConvertFns for RecipeBody<Function> {
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

impl ConvertFns for Authentication<Template<Function>> {
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

impl ConvertFns for Folder<Function> {
    type Output = Folder<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        Folder {
            id: self.id,
            name: self.name,
            children: self.children.convert(registry),
        }
    }
}

impl ConvertFns for Template<Function> {
    type Output = Template<FunctionId>;

    fn convert(self, registry: &mut FunctionRegistry) -> Self::Output {
        match self {
            Self::Value(s) => Template::Value(s),
            Self::Lazy(function) => {
                let id = registry.register(function);
                Template::Lazy(id)
            }
        }
    }
}
