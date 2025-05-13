//! Import from the legacy Slumber YAML format

mod cereal;
mod collection;
mod template;

use crate::{
    ImportCollection,
    common::{IntoPetitAst, Json, build_query_parameters, build_template},
    legacy::{
        collection::{
            self as legacy, Chain, ChainId, ChainOutputTrim,
            ChainRequestSection, ChainRequestTrigger, ChainSource,
            SelectOptions, SelectorMode,
        },
        template::{Template, TemplateInputChunk, TemplateKey},
    },
};
use anyhow::Context;
use indexmap::IndexMap;
use itertools::Itertools;
use petitscript::ast::{
    AstVisitor, Declaration, Expression, FunctionBody, FunctionCall,
    FunctionDefinition, Identifier, IntoExpression, IntoNode, ObjectLiteral,
    TemplateChunk, Walk,
};
use slumber_core::{
    collection::{self as core, RecipeTree},
    petit,
};
use slumber_util::parse_yaml;
use std::{
    collections::{HashSet, VecDeque},
    fs::File,
    hash::Hash,
    path::Path,
};
use tracing::{error, info};

const CHAIN_FN_PREFIX: &str = "chain_";

/// Convert a legacy Slumber YAML collection into the common import format
pub fn from_legacy(
    legacy_file: impl AsRef<Path>,
) -> anyhow::Result<ImportCollection> {
    let legacy_file = legacy_file.as_ref();
    info!(file = ?legacy_file, "Loading Slumber YAML collection");
    let file = File::open(legacy_file).context(format!(
        "Error opening Slumber YAML collection file {legacy_file:?}"
    ))?;
    // Since this is our own format, we're very strict about the import. If it
    // fails, that should be a fatal bug
    let collection: legacy::Collection = parse_yaml(file).context(format!(
        "Error deserializing Slumber YAML collection file {legacy_file:?}",
    ))?;

    let declarations = convert_chains(collection.chains);

    let profiles = collection
        .profiles
        .into_iter()
        .map(|(id, profile)| (id, profile.into()))
        .collect();

    // This will enforce ID uniqueness
    let recipes = RecipeTree::new(
        collection
            .recipes
            .into_iter()
            .map(|(id, node)| (id, node.into()))
            .collect(),
    )?;

    Ok(ImportCollection {
        declarations,
        profiles,
        recipes,
    })
}

/// Convert chains to functions. We have to map the whole collection together so
/// that the functions can be ordered by dependency.
fn convert_chains(chains: IndexMap<ChainId, Chain>) -> Vec<Declaration> {
    let chains = chains.into_values().map(Chain::into_ast).collect();
    let chains = sort_chains(chains);
    chains
        .into_iter()
        .map(|(identifier, definition)| definition.declare(identifier))
        .collect()
}

/// Sort a list of chain functions topologically, so that dependent functions
/// come after their dependencies. For example, if `b` calls `a()`, then `a()`
/// must come before `b()`.
fn sort_chains(
    chains: Vec<(Identifier, FunctionDefinition)>,
) -> Vec<(Identifier, FunctionDefinition)> {
    struct ChainFunction {
        name: Identifier,
        definition: FunctionDefinition,
        dependencies: Dependencies,
    }
    struct Dependencies(HashSet<Identifier>);

    /// Find all called chain functions in a single function body
    impl AstVisitor for Dependencies {
        fn enter_function_call(&mut self, call: &mut FunctionCall) {
            // unstable: if-let chain
            if let Expression::Identifier(identifier) = call.function.data() {
                if identifier.as_str().starts_with(CHAIN_FN_PREFIX) {
                    self.0.insert(identifier.data().clone());
                }
            }
        }
    }

    // Split the set of functions into ones that have no dependencies and are
    // ready to be declared, and ones that needs other dependencies declared
    // first.
    let (mut no_deps, mut has_deps): (VecDeque<_>, VecDeque<_>) = chains
        .into_iter()
        .map(|(name, mut definition)| {
            let mut dependencies = Dependencies(Default::default());
            definition.walk(&mut dependencies);
            ChainFunction {
                name,
                definition,
                dependencies,
            }
        })
        // Split it into (has no dependencies, has dependencies)
        .partition(|function| function.dependencies.0.is_empty());
    let mut output = Vec::with_capacity(no_deps.len() + has_deps.len());

    // https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm
    // We use VecDeque so we can iterate over in order of chain definition,
    // keeping the ordering stable where possible
    while let Some(function) = no_deps.pop_front() {
        let ChainFunction {
            name,
            definition,
            dependencies,
        } = function;
        debug_assert!(
            dependencies.0.is_empty(),
            "dependencies not empty for {name}"
        );

        // This function has no remaining dependencies, so we can add it to the
        // output. First though, remove its name from all dependents because
        // it's about to be declared
        for function in &mut has_deps {
            // shift_remove is "slow", but dependency lists will be extremely
            // small and this allows us to preserve order
            function.dependencies.0.remove(&name);
        }

        // Any function that no longer has undeclared dependencies gets moved
        // to the active queue. This is a scuffed alternative to extract_if,
        // because that doesn't exist on VecDeque
        has_deps = has_deps
            .into_iter()
            .filter_map(|function| {
                if function.dependencies.0.is_empty() {
                    // Move to the other queue
                    no_deps.push_back(function);
                    None
                } else {
                    // Keep it in this queue
                    Some(function)
                }
            })
            .collect();

        output.push((name, definition));
    }

    // If anything is still left in has_deps, that indicates a cycle. We should
    // include all those functions for completeness, but print an error
    // indicating that they won't work. This means the original chains had a
    // cycle too, and therefore never worked.
    if !has_deps.is_empty() {
        error!(
            "Cycle detected between chains: {}. The chains have been \
            converted to functions, but PetitScript does not support mutual \
            recursion so they will fail to run.",
            has_deps.iter().map(|function| &function.name).format(", ")
        );
        output.extend(
            has_deps
                .into_iter()
                .map(|function| (function.name, function.definition)),
        );
    }

    output
}

/// Generate a function name and definition. The function's execution will be
/// equivalent to evaluating this chain. We don't want to compose the parts into
/// a declaration yet, because it makes sorting by dependency harder.
impl IntoPetitAst for Chain {
    type Output = (Identifier, FunctionDefinition);

    fn into_ast(self) -> Self::Output {
        /// Build an arg list of `R` required arguments plus one keyword
        /// argument of `O` entries. Any empty kwargs will be omitted.
        /// If all kwargs are empty, omit the entire kwargs object
        fn with_kwargs<const R: usize, const KW: usize>(
            required: [Expression; R],
            kwargs: [(&str, Option<Expression>); KW],
        ) -> Vec<Expression> {
            let mut arguments: Vec<Expression> = required.into();
            let kwargs = kwargs
                .into_iter()
                .filter_map(|(k, v)| Some((k, v?)))
                .collect_vec();
            if !kwargs.is_empty() {
                arguments.push(ObjectLiteral::new(kwargs).into());
            }
            arguments
        }

        // Populate the function body according to the source. Start with a
        // single function call
        let is_prompt = matches!(&self.source, ChainSource::Prompt { .. });
        let mut body_expression = match self.source {
            ChainSource::Command { command, stdin } => petit::call_fn(
                "command",
                [command.into_ast().into()],
                [("stdin", stdin.map(Template::into_ast))],
            ),
            ChainSource::Environment { variable } => {
                petit::call_fn("env", [variable.into_ast()], [])
            }
            ChainSource::File { path } => {
                petit::call_fn("file", [path.into_ast()], [])
            }
            ChainSource::Prompt { message, default } => {
                petit::call_fn(
                    "prompt",
                    [],
                    [
                        ("message", message.map(Template::into_ast)),
                        ("default", default.map(Template::into_ast)),
                        // Only include this flag if it's enabled
                        ("sensitive", self.sensitive.then_some(true.into())),
                    ],
                )
            }
            ChainSource::Request {
                recipe,
                trigger,
                section: ChainRequestSection::Body,
            } => petit::call_fn(
                "response",
                [recipe.into_ast()],
                [("trigger", trigger.into_ast().map(Expression::from))],
            ),
            ChainSource::Request {
                recipe,
                trigger,
                section: ChainRequestSection::Header(header),
            } => petit::call_fn(
                "responseHeader",
                [recipe.into_ast(), header.into_ast()],
                [("trigger", trigger.into_ast().map(Expression::from))],
            ),
            ChainSource::Select { message, options } => petit::call_fn(
                "select",
                [options.into_ast()],
                [("message", message.map(Template::into_ast))],
            ),
        }
        .into_expr();

        // To replicate trimming, call the appropriate method from string's
        // prototype. This requires the expression to resolve to a string.
        match self.trim {
            ChainOutputTrim::None => {}
            ChainOutputTrim::Start => {
                body_expression = body_expression.call("trimStart", [])
            }
            ChainOutputTrim::End => {
                body_expression = body_expression.call("trimEnd", [])
            }
            ChainOutputTrim::Both => {
                body_expression = body_expression.call("trim", [])
            }
        };

        // Import selectors with a call to jsonpath()
        if let Some(selector) = self.selector {
            let mode = match self.selector_mode {
                SelectorMode::Auto => None, // This is default, so we can omit
                SelectorMode::Single => Some("single".into()),
                SelectorMode::Array => Some("array".into()),
            };
            // The only supported content type in external formats is JSON. We
            // need to manually parse to JSON here so we can query it. This
            // ends up looking like:
            // jsonPath('query', JSON.parse(body_expression))
            let json_parse_call = FunctionCall::new(
                Expression::reference("JSON").property("parse"),
                [body_expression],
            );
            let json_path_call = FunctionCall::named(
                "jsonPath",
                with_kwargs(
                    [selector.to_string().into(), json_parse_call.into()],
                    [("mode", mode)],
                ),
            );
            body_expression = json_path_call.into();
        }

        // Wrap the body in sensitive(). Skip this for prompts because they
        // have an equivalent kwarg so it's redundant. This must go last so
        // we're masking the final product, after all transformations
        if self.sensitive && !is_prompt {
            body_expression =
                FunctionCall::named("sensitive", [body_expression]).into();
        }

        let name = chain_id_to_function(&self.id);
        let identifier = FunctionDefinition::new(
            // Chains don't accept params, so the function won't either
            [],
            FunctionBody::expression(body_expression),
        );
        (name, identifier)
    }
}

impl From<legacy::Profile> for core::Profile<Expression> {
    fn from(profile: legacy::Profile) -> Self {
        core::Profile {
            id: profile.id,
            name: profile.name,
            default: profile.default,
            data: map_values(profile.data, Template::into_ast),
        }
    }
}

impl From<legacy::RecipeNode> for core::RecipeNode<Expression> {
    fn from(node: legacy::RecipeNode) -> Self {
        match node {
            legacy::RecipeNode::Folder(folder) => {
                core::RecipeNode::Folder(folder.into())
            }
            legacy::RecipeNode::Recipe(recipe) => {
                core::RecipeNode::Recipe(recipe.into())
            }
        }
    }
}

impl From<legacy::Folder> for core::Folder<Expression> {
    fn from(folder: legacy::Folder) -> Self {
        core::Folder {
            id: folder.id,
            name: folder.name,
            children: map_values(folder.children, core::RecipeNode::from),
        }
    }
}

impl From<legacy::Recipe> for core::Recipe<Expression> {
    fn from(recipe: legacy::Recipe) -> Self {
        core::Recipe {
            id: recipe.id,
            persist: recipe.persist,
            name: recipe.name,
            method: recipe.method,
            url: recipe.url.into_ast(),
            body: recipe.body.map(core::RecipeBody::from),
            authentication: recipe
                .authentication
                .map(core::Authentication::from),
            query: build_query_parameters(recipe.query),
            headers: map_values(recipe.headers, Template::into_ast),
        }
    }
}

impl From<legacy::Authentication> for core::Authentication<Expression> {
    fn from(authentication: legacy::Authentication) -> Self {
        match authentication {
            legacy::Authentication::Basic { username, password } => {
                core::Authentication::Basic {
                    username: username.into_ast(),
                    password: password
                        .map(Template::into_ast)
                        .unwrap_or_else(|| "".into()),
                }
            }
            legacy::Authentication::Bearer(token) => {
                core::Authentication::Bearer {
                    token: token.into_ast(),
                }
            }
        }
    }
}

impl From<legacy::RecipeBody> for core::RecipeBody<Expression> {
    fn from(body: legacy::RecipeBody) -> Self {
        match body {
            // Raw string body -> create a string or template
            legacy::RecipeBody::Raw(body) => core::RecipeBody::Raw {
                data: body.into_ast(),
            },
            legacy::RecipeBody::Json(json) => core::RecipeBody::Json {
                data: Json {
                    value: json,
                    // Convert each string to a template
                    convert_string: |s| {
                        // Theoretically the string should be a valid template,
                        // but if not treat it literally
                        match s.parse::<Template>() {
                            Ok(template) => template.into_ast(),
                            Err(_) => s.into(),
                        }
                    },
                }
                .into(),
            },
            legacy::RecipeBody::FormUrlencoded(fields) => {
                core::RecipeBody::FormUrlencoded {
                    data: map_values(fields, Template::into_ast),
                }
            }
            legacy::RecipeBody::FormMultipart(fields) => {
                core::RecipeBody::FormMultipart {
                    data: map_values(fields, Template::into_ast),
                }
            }
        }
    }
}

// TODO can we eliminate the usage of IntoAst?

impl IntoPetitAst for SelectOptions {
    type Output = Expression;

    /// Convert a static list of options into an array literal, or a dynamic
    /// template into an expression that will evaluate to an array
    fn into_ast(self) -> Self::Output {
        match self {
            // Array literal
            SelectOptions::Fixed(templates) => templates.into_ast().into(),
            // Single expression
            SelectOptions::Dynamic(template) => template.into_ast(),
        }
    }
}

impl IntoPetitAst for ChainRequestTrigger {
    type Output = Option<String>;

    /// Generate a string representing a trigger condition. Static conditions
    /// use static strings. The "expire" condition uses a duration string
    fn into_ast(self) -> Self::Output {
        match self {
            // The kwargs should be excluded if it's the default
            Self::Never => None,
            Self::NoHistory => Some("noHistory".into()),
            Self::Expire(duration) => Some(duration.to_string()),
            Self::Always => Some("always".into()),
        }
    }
}

impl IntoPetitAst for Template {
    type Output = Expression;

    /// Convert a legacy Slumber template to an expression. Empty and
    /// single-chunk templates will either become a string literal or a bare
    /// expression. Multi-chunk templates will be converted to a PS template
    /// literal.
    fn into_ast(self) -> Self::Output {
        let chunks = self.chunks.into_iter().map(TemplateInputChunk::into_ast);
        build_template(chunks)
    }
}

// TODO de-dupe with IntoPetitAst impl
impl From<Template> for Expression {
    fn from(template: Template) -> Self {
        template.into_ast()
    }
}

impl IntoPetitAst for TemplateInputChunk {
    type Output = TemplateChunk;

    fn into_ast(self) -> Self::Output {
        match self {
            TemplateInputChunk::Raw(s) => TemplateChunk::Literal(s),
            TemplateInputChunk::Key(key) => {
                TemplateChunk::Expression(key.into_ast().into_expr().s())
            }
        }
    }
}

impl IntoPetitAst for TemplateKey {
    type Output = FunctionCall;

    /// Generate an expression corresponding to a dynamic template key
    fn into_ast(self) -> Self::Output {
        match self {
            // `{{field1}}` -> `profile('field1')`
            TemplateKey::Field(identifier) => {
                petit::profile_field(identifier.to_string())
            }
            // `{{chains.chain1}}` -> `chain_chain1()`
            // Chain functions are always nullary because old chain references
            // had no way of passing arguments
            TemplateKey::Chain(chain_id) => {
                FunctionCall::named(chain_id_to_function(&chain_id), [])
            }
            // `{{env.VAR1}}` -> `env('VAR1')`
            TemplateKey::Environment(identifier) => {
                FunctionCall::named("env", [identifier.to_string().into_expr()])
            }
        }
    }
}

/// Get a function name from a chain ID
fn chain_id_to_function(chain_id: &ChainId) -> Identifier {
    // TODO escape ID so parsing can never fail
    Identifier::try_from(format!("{CHAIN_FN_PREFIX}{}", chain_id.0)).unwrap()
}

/// Apply a transformation function to each element in a map
fn map_values<K, V1, V2>(
    map: IndexMap<K, V1>,
    f: impl Fn(V1) -> V2,
) -> IndexMap<K, V2>
where
    K: Eq + Hash + PartialEq,
{
    map.into_iter()
        .map(|(key, value)| (key, f(value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_core::{
        petit::{ENGINE, call_fn},
        test_util::by_id,
    };
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const YAML_FILE: &str = "legacy.yml";
    /// Assertion expectation is stored in a separate file. This is for a couple
    /// reasons:
    /// - It's huge so it makes code hard to navigate
    /// - Changes don't require a re-compile
    const YAML_EXPECTED_FILE: &str = "legacy_expected.js";

    #[rstest]
    fn test_legacy_import(test_data_dir: PathBuf) {
        // Convert the external collection into a PS AST, then parse the
        // expected file into an AST and compare the two
        let imported = from_legacy(test_data_dir.join(YAML_FILE))
            .unwrap()
            .into_petitscript();
        let expected = ENGINE
            .parse(test_data_dir.join(YAML_EXPECTED_FILE))
            .unwrap();
        assert_eq!(&imported, expected.data());
    }

    /// Chains are reordered according to their dependency (topologically
    /// sorted)
    #[rstest]
    fn test_chain_reorder() {
        // Dependency chain is chain1 -> chain2 -> chain3
        let chains = by_id([
            prompt("chain1", "{{chains.chain2}}"),
            prompt("chain2", "{{chains.chain3}}"),
            prompt("chain3", "the end"),
        ]);
        let actual = convert_chains(chains);
        let expected = vec![
            // Functions have been reversed to match the topologically ordering
            call_prompt("chain_chain3", "the end"),
            call_prompt(
                "chain_chain2",
                FunctionCall::named("chain_chain3", []),
            ),
            call_prompt(
                "chain_chain1",
                FunctionCall::named("chain_chain2", []),
            ),
        ];
        assert_eq!(actual, expected);
    }

    /// Chains with a dependency cycle are all included, but there's no
    /// consistent ordering
    #[rstest]
    fn test_chain_cycle() {
        // Dependency chain is chain1 -> chain2 -> chain3 -> chain1
        // Since there's no consistent ordering, we keep the input order
        let chains = by_id([
            prompt("chain1", "{{chains.chain2}}"),
            prompt("chain2", "{{chains.chain3}}"),
            prompt("chain3", "{{chains.chain1}}"),
        ]);
        let actual = convert_chains(chains);
        let expected = vec![
            call_prompt(
                "chain_chain1",
                FunctionCall::named("chain_chain2", []),
            ),
            call_prompt(
                "chain_chain2",
                FunctionCall::named("chain_chain3", []),
            ),
            call_prompt(
                "chain_chain3",
                FunctionCall::named("chain_chain1", []),
            ),
        ];
        assert_eq!(actual, expected);
    }

    /// Build a prompt chain
    fn prompt(id: &'static str, message: &'static str) -> Chain {
        Chain {
            id: id.into(),
            source: ChainSource::Prompt {
                message: Some(message.into()),
                default: None,
            },
            sensitive: false,
            selector: None,
            selector_mode: SelectorMode::Auto,
            trim: ChainOutputTrim::None,
            _content_type: serde::de::IgnoredAny,
        }
    }

    /// Define a chain function that calls a prompt
    fn call_prompt(
        name: &'static str,
        message: impl Into<Expression>,
    ) -> Declaration {
        FunctionDefinition::new(
            [],
            FunctionBody::expression(call_fn(
                "prompt",
                [],
                [("message", Some(message.into()))],
            )),
        )
        .declare(name)
    }
}
