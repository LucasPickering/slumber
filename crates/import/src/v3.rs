mod models;
mod template;

use crate::{
    ImportInput, common,
    v3::models::{Chain, ChainId, ChainRequestSection},
};
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use models as v3;
use slumber_core::collection as v4;
use slumber_template::{Expression, TemplateChunk};
use std::hash::Hash;

/// Import from the Slumber v3 collection format. The major changes:
/// - Replace chains with a function-based template language
/// - Removal of YAML !tags, in favor of an internal `type` field
/// - Replace YAML anchor/alias/merge with `$ref`
pub async fn from_v3(input: &ImportInput) -> anyhow::Result<v4::Collection> {
    let content = input.load().await?;
    // Deserialize v3
    let collection: v3::Collection = serde_yaml::from_str(&content)
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
impl IntoV4 for Vec<(String, template::Template)> {
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
                object.into_v4(chains).map(v4::JsonTemplate::Object)
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
impl IntoV4 for template::Template {
    type Output = slumber_template::Template;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        let chunks = self.chunks.into_v4(chains)?;
        Ok(slumber_template::Template { chunks })
    }
}

/// Convert each chunk of the template: raw to raw, key to expression
impl IntoV4 for template::TemplateInputChunk {
    type Output = slumber_template::TemplateChunk;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Raw(s) => Ok(TemplateChunk::Raw(s)),
            Self::Key(key) => {
                key.into_v4(chains).map(TemplateChunk::Expression)
            }
        }
    }
}

/// Convert a dynamic key to an expression
impl IntoV4 for template::TemplateKey {
    type Output = slumber_template::Expression;

    fn into_v4(
        self,
        chains: &IndexMap<ChainId, Chain>,
    ) -> anyhow::Result<Self::Output> {
        match self {
            Self::Field(field) => field
                .into_v4(chains)
                .map(slumber_template::Expression::Field),
            Self::Environment(variable) => {
                // Env variables are accessed through a template now
                Ok(slumber_template::Expression::call(
                    "env",
                    [variable.0.into()],
                    [],
                ))
            }
            Self::Chain(chain_id) => {
                // This is the big boy: translate a chain to a function call.
                // We're going to build up a function-based expression, with
                // a call on the left then zero or more calls piped after that
                let chain = chains
                    .get(&chain_id)
                    .ok_or(anyhow!("Unknown chain `{}`", chain_id.0.0))?;

                // First function comes from the data source
                let mut expression = match chain.source {
                    v3::ChainSource::Command { command, stdin } => {
                        Expression::call(
                            "command",
                            [command.into()],
                            // TODO make this optional
                            [("stdin", stdin.into())],
                        )
                    }
                    v3::ChainSource::Environment { variable } => {
                        slumber_template::Expression::call(
                            "env",
                            [variable.into()],
                            [],
                        )
                    }
                    v3::ChainSource::File { path } => {
                        slumber_template::Expression::call(
                            "file",
                            [path.into()],
                            [],
                        )
                    }
                    v3::ChainSource::Prompt { message, default } => {
                        slumber_template::Expression::call(
                            "prompt",
                            [],
                            [
                                ("message", message.into()),
                                ("default", default.into()),
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
                    } => slumber_template::Expression::call(
                        "response",
                        [recipe.into()],
                        [("trigger", trigger.into())],
                    ),
                    v3::ChainSource::Request {
                        recipe,
                        trigger,
                        section: ChainRequestSection::Header(header),
                    } => slumber_template::Expression::call(
                        "response_header",
                        [recipe.into(), header.into()],
                        [("trigger", trigger.into())],
                    ),
                    v3::ChainSource::Select { message, options } => {
                        slumber_template::Expression::call(
                            "select",
                            [options.into()],
                            [("message", message.into())],
                        )
                    }
                };

                // Apply additional filters
                // TODO make sure these are all tested
                if let Some(selector) = chain.selector {
                    let mode = match chain.selector_mode {
                        // Omit if default
                        v3::SelectorMode::Auto => None,
                        v3::SelectorMode::Single => Some("single".into()),
                        v3::SelectorMode::Array => Some("array".into()),
                    };
                    expression = expression.pipe(
                        "jsonpath",
                        [selector.to_string().into()],
                        // TODO omit if default
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
                // Sensitive goes last because it would mess with other ops
                if chain.sensitive {
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
