mod models;
mod template;

use crate::{
    ImportInput, common,
    v3::{
        models::{
            Chain, ChainId, ChainRequestSection, ChainRequestTrigger,
            ChainSource, SelectOptions,
        },
        template::{
            Template as TemplateV3, TemplateInputChunk as TemplateInputChunkV3,
        },
    },
};
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use models as v3;
use slumber_core::collection as v4;
use slumber_template::{
    Expression, Template as TemplateV4, TemplateChunk as TemplateChunkV4,
};
use slumber_util::{TimeSpan, yaml::SourceLocation};
use std::{hash::Hash, ops::Deref};

/// Import from the Slumber v3 collection format. The major changes:
/// - Replace chains with a function-based template language
/// - Removal of YAML !tags, in favor of an internal `type` field
/// - Replace YAML anchor/alias/merge with `$ref`
pub async fn from_v3(input: &ImportInput) -> anyhow::Result<v4::Collection> {
    let content = input.load().await?;

    // We use two-step parsing to enable pre-processing on the YAML
    let mut yaml_value: serde_yaml::Value = serde_yaml::from_str(&content)?;

    // Merge anchors+aliases
    yaml_value.apply_merge()?;
    // Remove any top-level fields that start with .
    if let serde_yaml::Value::Mapping(mapping) = &mut yaml_value {
        mapping.retain(|key, _| {
            !key.as_str().is_some_and(|key| key.starts_with('.'))
        });
    }

    // Deserialize v3
    let collection: v3::Collection = serde_yaml::from_value(yaml_value)
        .context("Error deserializing v3 collection")?;

    // Convert stuff
    let profiles = collection.profiles.into_v4(&collection.chains)?;
    let recipes = collection.recipes.into_v4(&collection.chains)?;
    // Collect into a tree. This is where dupe IDs will be caught
    let recipes = v4::RecipeTree::new(recipes)?;

    Ok(v4::Collection {
        name: collection.name,
        profiles,
        recipes,
    })
}

/// Convert a v3 object into its v4 equivalent
trait IntoV4 {
    type Output;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output>;
}

impl<T: IntoV4> IntoV4 for Vec<T> {
    type Output = Vec<T::Output>;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        self.into_iter().map(|v| v.into_v4(chains)).collect()
    }
}

impl<K, V> IntoV4 for IndexMap<K, V>
where
    K: Eq + Hash + PartialEq,
    V: IntoV4,
{
    type Output = IndexMap<K, V::Output>;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        self.into_iter()
            .map(|(k, v)| Ok((k, v.into_v4(chains)?)))
            .collect()
    }
}

impl<T: IntoV4> IntoV4 for Option<T> {
    type Output = Option<T::Output>;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        self.map(|v| v.into_v4(chains)).transpose()
    }
}

impl IntoV4 for v3::Profile {
    type Output = v4::Profile;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        let data = self.data.into_v4(chains)?;
        Ok(v4::Profile {
            id: self.id,
            location: SourceLocation::default(),
            name: self.name,
            default: self.default,
            data,
        })
    }
}

impl IntoV4 for v3::RecipeNode {
    type Output = v4::RecipeNode;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Folder(folder) => {
                Ok(v4::RecipeNode::Folder(folder.into_v4(chains)?))
            }
            Self::Recipe(recipe) => {
                Ok(v4::RecipeNode::Recipe(recipe.into_v4(chains)?))
            }
        }
    }
}

impl IntoV4 for v3::Folder {
    type Output = v4::Folder;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        Ok(v4::Folder {
            id: self.id,
            location: SourceLocation::default(),
            name: self.name,
            children: self.children.into_v4(chains)?,
        })
    }
}

/// Convert a recipe from v3 to v4
impl IntoV4 for v3::Recipe {
    type Output = v4::Recipe;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        Ok(v4::Recipe {
            id: self.id,
            location: SourceLocation::default(),
            persist: self.persist,
            name: self.name,
            method: self.method,
            url: self.url.into_v4(chains)?,
            body: self.body.into_v4(chains)?,
            authentication: self.authentication.into_v4(chains)?,
            query: self.query.into_v4(chains)?,
            headers: self.headers.into_v4(chains)?,
        })
    }
}

/// Convert query params
impl IntoV4 for Vec<(String, TemplateV3)> {
    type Output = IndexMap<String, v4::QueryParameterValue>;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        let converted = self
            .into_iter()
            .map(|(param, template)| Ok((param, template.into_v4(chains)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(common::build_query_parameters(converted))
    }
}

impl IntoV4 for v3::RecipeBody {
    type Output = v4::RecipeBody;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Raw(template) => {
                Ok(v4::RecipeBody::Raw(template.into_v4(chains)?))
            }
            Self::Json(json) => Ok(v4::RecipeBody::Json(json.into_v4(chains)?)),
            Self::FormUrlencoded(form) => {
                Ok(v4::RecipeBody::FormUrlencoded(form.into_v4(chains)?))
            }
            Self::FormMultipart(form) => {
                Ok(v4::RecipeBody::FormMultipart(form.into_v4(chains)?))
            }
        }
    }
}

impl IntoV4 for v3::JsonTemplate {
    type Output = v4::JsonTemplate;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Null => Ok(v4::JsonTemplate::Null),
            Self::Bool(b) => Ok(v4::JsonTemplate::Bool(b)),
            Self::Number(number) => Ok(v4::JsonTemplate::Number(number)),
            Self::String(template) => {
                template.into_v4(chains).map(v4::JsonTemplate::String)
            }
            Self::Array(array) => {
                array.into_v4(chains).map(v4::JsonTemplate::Array)
            }
            Self::Object(object) => {
                let entries = object
                    .into_v4(chains)?
                    .into_iter()
                    // Keys are plain strings in v3 but templates in v4. Escape
                    // the keys instead of parsing to retain the same behavior
                    .map(|(k, v)| (slumber_template::Template::raw(k), v))
                    .collect();
                Ok(v4::JsonTemplate::Object(entries))
            }
        }
    }
}

impl IntoV4 for v3::Authentication {
    type Output = v4::Authentication;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Basic { username, password } => {
                Ok(v4::Authentication::Basic {
                    username: username.into_v4(chains)?,
                    password: password.into_v4(chains)?,
                })
            }
            Self::Bearer(token) => Ok(v4::Authentication::Bearer {
                token: token.into_v4(chains)?,
            }),
        }
    }
}

/// Convert an entire template from chains to functions
impl IntoV4 for TemplateV3 {
    type Output = TemplateV4;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        self.chunks.into_v4(chains).map(TemplateV4::from_chunks)
    }
}

/// Convert each chunk of the template: raw to raw, key to expression
impl IntoV4 for TemplateInputChunkV3 {
    type Output = TemplateChunkV4;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Raw(s) => Ok(TemplateChunkV4::Raw(s)),
            Self::Key(key) => {
                key.into_v4(chains).map(TemplateChunkV4::Expression)
            }
        }
    }
}

/// Convert a dynamic key to an expression
impl IntoV4 for template::TemplateKey {
    type Output = Expression;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Field(field) => field.into_v4(chains).map(Expression::Field),
            Self::Environment(variable) => {
                // Env variables are accessed through a template now
                Ok(Expression::call("env", [variable.0.into()], []))
            }
            Self::Chain(chain_id) => {
                // This is the big boy: translate a chain to a function call.
                // We're going to build up a function-based expression, with
                // a call on the left then zero or more calls piped after that
                let chain = chains
                    .get(&chain_id)
                    .ok_or(anyhow!("Unknown chain `{}`", chain_id.0.0))?;

                // First function comes from the data source
                let mut expression = match &chain.source {
                    v3::ChainSource::Command { command, stdin } => {
                        Expression::call(
                            "command",
                            [command
                                .clone()
                                .into_iter()
                                .map(|template| {
                                    template.try_into_expression(chains)
                                })
                                .collect::<anyhow::Result<_>>()?],
                            [(
                                "stdin",
                                stdin
                                    .clone()
                                    .map(|template| {
                                        template.try_into_expression(chains)
                                    })
                                    .transpose()?,
                            )],
                        )
                    }
                    v3::ChainSource::Environment { variable } => {
                        Expression::call(
                            "env",
                            [variable.clone().try_into_expression(chains)?],
                            [],
                        )
                    }
                    v3::ChainSource::File { path } => Expression::call(
                        "file",
                        [path.clone().try_into_expression(chains)?],
                        [],
                    ),
                    v3::ChainSource::Prompt { message, default } => {
                        Expression::call(
                            "prompt",
                            [],
                            [
                                (
                                    "message",
                                    message
                                        .clone()
                                        .map(|template| {
                                            template.try_into_expression(chains)
                                        })
                                        .transpose()?,
                                ),
                                (
                                    "default",
                                    default
                                        .clone()
                                        .map(|template| {
                                            template.try_into_expression(chains)
                                        })
                                        .transpose()?,
                                ),
                                (
                                    // Mask the input if marked as sensitive
                                    "sensitive",
                                    if chain.sensitive {
                                        Some(true.into())
                                    } else {
                                        None
                                    },
                                ),
                            ],
                        )
                    }
                    v3::ChainSource::Request {
                        recipe,
                        trigger,
                        section: ChainRequestSection::Body,
                    } => Expression::call(
                        "response",
                        // Recipe ID is *not* a template in v3
                        [recipe.to_string().into()],
                        [("trigger", trigger.to_expression())],
                    ),
                    v3::ChainSource::Request {
                        recipe,
                        trigger,
                        section: ChainRequestSection::Header(header),
                    } => Expression::call(
                        "response_header",
                        // Recipe ID is *not* a template in v3
                        [
                            recipe.to_string().into(),
                            header.clone().try_into_expression(chains)?,
                        ],
                        [("trigger", trigger.to_expression())],
                    ),
                    v3::ChainSource::Select { message, options } => {
                        Expression::call(
                            "select",
                            [options.clone().into_v4(chains)?],
                            [(
                                "message",
                                message
                                    .clone()
                                    .map(|template| {
                                        template.try_into_expression(chains)
                                    })
                                    .transpose()?,
                            )],
                        )
                    }
                };

                // Apply additional filters
                if let Some(selector) = &chain.selector {
                    let mode = match chain.selector_mode {
                        // Omit if default
                        v3::SelectorMode::Auto => None,
                        v3::SelectorMode::Single => Some("single".into()),
                        v3::SelectorMode::Array => Some("array".into()),
                    };
                    expression = expression.pipe(
                        "jsonpath",
                        [selector.to_string().into()],
                        [("mode", mode)],
                    );
                }
                expression = match chain.trim {
                    v3::ChainOutputTrim::None => expression,
                    v3::ChainOutputTrim::Start => expression.pipe(
                        "trim",
                        [],
                        [("mode", Some("start".into()))],
                    ),
                    v3::ChainOutputTrim::End => expression.pipe(
                        "trim",
                        [],
                        [("mode", Some("end".into()))],
                    ),
                    // `both` is the default mode, no need to pass it
                    v3::ChainOutputTrim::Both => {
                        expression.pipe("trim", [], [])
                    }
                };
                // Sensitive goes last because it would mess with other ops.
                // This should be omitted for prompt() because it has its own
                // sensitive= keyword that already masks the output
                if chain.sensitive
                    && !matches!(&chain.source, ChainSource::Prompt { .. })
                {
                    expression = expression.pipe("sensitive", [], []);
                }

                Ok(expression)
            }
        }
    }
}

impl IntoV4 for template::Identifier {
    type Output = slumber_template::Identifier;

    fn into_v4(
        self,
        _chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        // This conversion should be 1:1, but make sure we adhere to v4
        // identifier rules
        self.0.parse().map_err(anyhow::Error::from)
    }
}

impl ChainRequestTrigger {
    /// Convert a request `trigger` value into the equivalent v4 expression.
    /// Return `None` for the default value (`never`)
    fn to_expression(self) -> Option<Expression> {
        match self {
            // This is the default so omit it
            ChainRequestTrigger::Never => None,
            ChainRequestTrigger::NoHistory => Some("no_history".into()),
            // We don't know the format of the original string, but this will
            // reduce large durations like `43200s` to `12h`
            ChainRequestTrigger::Expire(duration) => {
                Some(TimeSpan::from(duration).to_string().into())
            }
            ChainRequestTrigger::Always => Some("always".into()),
        }
    }
}

impl IntoV4 for SelectOptions {
    type Output = Expression;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            // We have a static list of expressions - map each individal item
            SelectOptions::Fixed(templates) => templates
                .into_iter()
                .map(|template| template.try_into_expression(chains))
                .collect(),
            // The list is generated from a template. Map that to an expression,
            // which we expect to evaluate to an array
            SelectOptions::Dynamic(template) => {
                template.try_into_expression(chains)
            }
        }
    }
}

impl TemplateV3 {
    /// Convert this template into a nested expression. This is for templates
    /// that appear within chain config. The template will be used as a function
    /// argument within a larger v4 template, so it can't become a template
    /// itself. Empty templates map to the empty string. Single-chunk templates
    /// map to their equivalent expression. Multi-chunk templates have to be
    /// manually concatenated using the `concat` function.
    fn try_into_expression(
        mut self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Expression> {
        match self.chunks.len() {
            0 => Ok("".into()),
            1 => match self.chunks.pop().unwrap() {
                TemplateInputChunkV3::Raw(s) => Ok(s.deref().into()),
                TemplateInputChunkV3::Key(key) => key.into_v4(chains),
            },
            // v4 doesn't support nested templates as function args so we need
            // to stitch the chunks together with concat():
            // field: "my name is {{name}}" -> concat(["my name is ", name])
            _ => {
                // Map each element, then collect into a vec so we can detect
                // errors
                let elements: Vec<Expression> = self
                    .chunks
                    .into_iter()
                    .map(|chunk| match chunk {
                        TemplateInputChunkV3::Raw(s) => Ok(s.deref().into()),
                        TemplateInputChunkV3::Key(key) => key.into_v4(chains),
                    })
                    .collect::<anyhow::Result<_>>()?;
                Ok(Expression::call("concat", [elements.into()], []))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const V3_FILE: &str = "v3.yml";
    const V3_IMPORTED_FILE: &str = "v3_imported.yml";

    /// Catch-all test for v3->v4 import. This uses the old v3 regression.yml
    /// and its v4 equivalent.
    #[rstest]
    #[tokio::test]
    async fn test_v3_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(V3_FILE));
        let imported = from_v3(&input).await.unwrap();
        let expected =
            v4::Collection::load(&test_data_dir.join(V3_IMPORTED_FILE))
                .unwrap();
        assert_eq!(imported, expected);
    }
}
