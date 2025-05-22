//! Import request collections from Insomnia. Based on the Insomnia v4 export
//! format

use crate::{ImportInput, common};
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use itertools::Itertools;
use mime::Mime;
use reqwest::header;
use serde::{Deserialize, Deserializer, de::Error as _};
use slumber_core::{
    collection::{
        self, Chain, ChainId, ChainSource, Collection, Folder, HasId, Profile,
        ProfileId, Recipe, RecipeBody, RecipeId, RecipeNode, RecipeTree,
        SelectorMode,
    },
    http::HttpMethod,
    template::{Identifier, Template},
};
use slumber_util::NEW_ISSUE_LINK;
use std::{collections::HashMap, fmt::Display, str::FromStr};
use tracing::{debug, error, warn};

/// Convert an Insomnia exported collection into the slumber format. This
/// supports YAML *or* JSON input.
///
/// This is not async because it's only called by the CLI, where we don't
/// care about blocking. It keeps the code simpler.
pub async fn from_insomnia(input: &ImportInput) -> anyhow::Result<Collection> {
    // First, deserialize into the insomnia format
    warn!(
        "The Insomnia importer is approximate. Some features are missing \
            and it most likely will not give you an equivalent collection. If \
            you would like to request support for a particular Insomnia \
            feature, please open an issue: {NEW_ISSUE_LINK}"
    );

    // The format can be YAML or JSON, so we can just treat it all as YAML
    let content = input.load().await?;
    let mut insomnia: Insomnia = serde_yaml::from_str(&content)
        .context("Error deserializing Insomnia collection")?;

    // Match Insomnia's visual order. This isn't entirely accurate because
    // Insomnia reorders folders/requests according to the tree structure,
    // but it should get us the right order within each layer
    insomnia.resources.sort_by_key(Resource::sort_key);

    let Grouped {
        workspace_id,
        environments,
        request_groups,
        requests,
    } = Grouped::group(insomnia)?;

    // Convert everything we care about
    let profiles = build_profiles(&workspace_id, environments);
    let chains = build_chains(&requests);
    let recipes = build_recipe_tree(&workspace_id, request_groups, requests)?;

    Ok(Collection {
        name: None,
        profiles,
        recipes,
        chains,
    })
}

#[derive(Debug, Deserialize)]
struct Insomnia {
    resources: Vec<Resource>,
}

/// Group the resources by type so they're easier to access
struct Grouped {
    /// Root workspace ID. Useful to check which resources are top-level and
    /// which aren't.
    workspace_id: String,
    /// Profiles
    environments: Vec<Environment>,
    /// Folders
    request_groups: Vec<RequestGroup>,
    /// Recipes
    requests: Vec<Request>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "_type", rename_all = "snake_case")]
enum Resource {
    /// Maps to a folder
    RequestGroup(RequestGroup),
    /// Maps to a recipe
    Request(Request),
    /// Maps to a profile
    Environment(Environment),
    Workspace {
        #[serde(rename = "_id")]
        id: String,
    },
    ApiSpec,
    /// Catch-all for unknown variants
    #[serde(untagged)]
    Other {
        #[serde(rename = "_id")]
        id: String,
        #[serde(rename = "_type")]
        kind: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Environment {
    #[serde(rename = "_id")]
    id: String,
    parent_id: String,
    name: String,
    data: IndexMap<String, String>,
    meta_sort_key: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestGroup {
    #[serde(rename = "_id")]
    id: String,
    parent_id: String,
    name: String,
    meta_sort_key: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    #[serde(rename = "_id")]
    id: String,
    parent_id: String,
    meta_sort_key: i64,
    name: String,
    url: String,
    method: HttpMethod,
    #[serde(deserialize_with = "deserialize_shitty_option")]
    authentication: Option<Authentication>,
    headers: Vec<Header>,
    parameters: Vec<Parameter>,
    #[serde(deserialize_with = "deserialize_shitty_option")]
    body: Option<Body>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Authentication {
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    /// Catch-all for unknown variants
    #[serde(untagged)]
    Other {
        #[serde(rename = "type")]
        kind: String,
    },
}

#[derive(Debug, Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    #[serde(deserialize_with = "deserialize_from_str")]
    mime_type: Mime,
    /// This field is only present for text-like bodies (e.g. *not* forms)
    #[serde(default)]
    text: Option<String>,
    /// Present for form-like bodies
    #[serde(default)]
    params: Vec<FormParam>,
}

impl Body {
    fn try_text(self) -> anyhow::Result<String> {
        self.text
            .ok_or_else(|| anyhow!("Body missing `text` field"))
    }
}

/// One parameter in a form (urlencoded or multipart)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormParam {
    id: String,
    name: String,
    value: String,
    /// Variant of param. This is omitted by Insomnia for simple text params,
    /// so we need to fill with default
    #[serde(default, rename = "type")]
    kind: FormParamKind,
    /// Path of linked file, for file params only
    file_name: Option<String>,
}

/// The variant of a form parameter
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FormParamKind {
    #[default]
    String,
    File,
}

impl Grouped {
    /// Group resources by type and throw away what we don't need
    fn group(insomnia: Insomnia) -> anyhow::Result<Self> {
        let mut workspace_id = None;
        let mut environments = Vec::new();
        let mut request_groups = Vec::new();
        let mut requests = Vec::new();

        for resource in insomnia.resources {
            match resource {
                Resource::Environment(environment) => {
                    environments.push(environment);
                }
                Resource::RequestGroup(request_group) => {
                    request_groups.push(request_group);
                }
                Resource::Request(request) => {
                    requests.push(request);
                }
                Resource::Workspace { id } => workspace_id = Some(id),
                // These are known types but we don't need to do anything
                Resource::ApiSpec => {}
                // Anything unknown should give a warning
                Resource::Other { id, kind } => {
                    error!("Ignoring resource `{id}` of unknown type `{kind}`");
                }
            }
        }

        Ok(Self {
            workspace_id: workspace_id
                .ok_or_else(|| anyhow!("Workspace resource not found"))?,
            environments,
            request_groups,
            requests,
        })
    }
}

impl Resource {
    /// Rather than order things how they should be, Insomnia attaches a sort
    /// key to each item
    fn sort_key(&self) -> i64 {
        match self {
            Resource::RequestGroup(folder) => folder.meta_sort_key,
            Resource::Request(request) => request.meta_sort_key,
            Resource::Environment(environment) => environment.meta_sort_key,
            Resource::Workspace { .. }
            | Resource::ApiSpec
            | Resource::Other { .. } => 0,
        }
    }
}

impl From<RequestGroup> for RecipeNode {
    fn from(folder: RequestGroup) -> Self {
        RecipeNode::Folder(Folder {
            id: folder.id.into(),
            name: Some(folder.name),
            // This will be populated later
            children: IndexMap::new(),
        })
    }
}

impl From<Request> for RecipeNode {
    fn from(request: Request) -> Self {
        let mut headers: IndexMap<String, Template> = IndexMap::new();

        // Preload headers from implicit sources
        if let Some(Body { mime_type, .. }) = &request.body {
            headers.insert(
                header::CONTENT_TYPE.as_str().into(),
                Template::raw(mime_type.to_string()),
            );
        }
        // Load explicit headers *after* so we can override the implicit stuff
        for header in request.headers {
            headers.insert(
                header.name.to_lowercase(),
                Template::raw(header.value),
            );
        }
        headers.shift_remove(header::USER_AGENT.as_str());

        let body = request
            .body
            .map(RecipeBody::try_from)
            .transpose()
            .inspect_err(|error| {
                error!(
                    "Error importing body for request `{id}`: {error}",
                    id = request.id
                );
            })
            .ok()
            .flatten();

        // Load authentication scheme
        let authentication =
            request.authentication.and_then(|authentication| {
                let result = authentication.try_into();
                if let Err(kind) = &result {
                    error!(
                        "Ignoring authentication of unknown type `{kind}` \
                        for request `{}`",
                        request.id
                    );
                }
                result.ok()
            });

        RecipeNode::Recipe(Recipe {
            id: request.id.into(),
            persist: true,
            name: Some(request.name),
            method: request.method,
            url: Template::raw(request.url),
            body,
            query: common::build_query_parameters(
                request.parameters.into_iter().map(|parameter| {
                    (parameter.name, Template::raw(parameter.value))
                }),
            ),
            headers,
            authentication,
        })
    }
}

impl TryFrom<Body> for RecipeBody {
    type Error = anyhow::Error;

    fn try_from(body: Body) -> anyhow::Result<Self> {
        let body = if body.mime_type == mime::APPLICATION_JSON {
            // Parse the body as JSON
            let json =
                serde_json::from_str(&body.try_text()?).with_context(|| {
                    format!(
                        "Body has MIME type `{}` but failed to parse as JSON",
                        mime::APPLICATION_JSON
                    )
                })?;
            RecipeBody::untemplated_json(json)
        } else if body.mime_type == mime::APPLICATION_WWW_FORM_URLENCODED {
            RecipeBody::FormUrlencoded(
                body.params.into_iter().map(FormParam::into).collect(),
            )
        } else if body.mime_type == mime::MULTIPART_FORM_DATA {
            RecipeBody::FormMultipart(
                body.params.into_iter().map(FormParam::into).collect(),
            )
        } else {
            RecipeBody::Raw(Template::raw(body.try_text()?))
        };
        Ok(body)
    }
}

/// Convert an Insomnia form parameter into a corresponding map entry, to be
/// used in a structured body
impl From<FormParam> for (String, Template) {
    fn from(param: FormParam) -> Self {
        match param.kind {
            // Simple string, map to a raw template
            FormParamKind::String => (param.name, Template::raw(param.value)),
            // We'll map this to a chain that loads the file. The ID of the
            // chain is the ID of this param. We're banking on that chain being
            // created elsewhere. It's a bit spaghetti but otherwise we'd need
            // mutable access to the entire collection, which I think would end
            // up with even more spaghetti
            FormParamKind::File => (
                param.name,
                Template::from_chain(Identifier::escape(&param.id).into()),
            ),
        }
    }
}

/// Convert authentication type. If the type is unknown, return is as `Err`
impl TryFrom<Authentication> for collection::Authentication {
    type Error = String;

    fn try_from(authentication: Authentication) -> Result<Self, Self::Error> {
        match authentication {
            Authentication::Basic { username, password } => {
                Ok(collection::Authentication::Basic {
                    username: Template::raw(username),
                    password: Some(Template::raw(password)),
                })
            }
            Authentication::Bearer { token } => {
                Ok(collection::Authentication::Bearer(Template::raw(token)))
            }
            // Caller should print a warning for this
            Authentication::Other { kind } => Err(kind),
        }
    }
}

/// Convert environments into profiles
fn build_profiles(
    workspace_id: &str,
    mut environments: Vec<Environment>,
) -> IndexMap<ProfileId, Profile> {
    fn convert_data(
        data: IndexMap<String, String>,
    ) -> impl Iterator<Item = (String, Template)> {
        data.into_iter().map(|(k, v)| (k, Template::raw(v)))
    }

    // The Base Environment is the one with the workspace as a parent. We
    // generally expect this to be present, but it's not fatal if it's missing.
    // It's generally also the first in the list but don't make any assumptions
    // about that
    let base_index = environments
        .iter()
        .position(|environment| environment.parent_id == workspace_id);
    let base_data: IndexMap<String, Template> = base_index
        .map(|i| {
            let environment = environments.remove(i);
            convert_data(environment.data).collect()
        })
        .unwrap_or_default();

    environments
        .into_iter()
        .map(|environment| {
            debug!("Generating profile for environment `{}`", environment.id);
            let id: ProfileId = environment.id.into();
            // Start with base data so we can overwrite it
            let data = base_data
                .clone()
                .into_iter()
                .chain(convert_data(environment.data))
                .collect();
            (
                id.clone(),
                Profile {
                    id,
                    name: Some(environment.name),
                    default: false,
                    data,
                },
            )
        })
        .collect()
}

/// Build up all the chains we need to represent the Insomnia collection.
/// Chains don't map 1:1 with any Insomnia resource. They generally are an
/// explicit representation of some implicit Insomnia behavior, so we have to
/// crawl over the Insomnia collection to find where chains need to exist. For
/// each generated chain, we'll need to pick a consistent ID so the consumer can
/// link to the same chain.
fn build_chains(requests: &[Request]) -> IndexMap<ChainId, Chain> {
    let mut chains = IndexMap::new();

    for request in requests {
        debug!("Generating chains for request `{}`", request.id);

        // Any multipart form param that references a file needs a chain
        for param in request.body.iter().flat_map(|body| &body.params) {
            debug!("Generating chains for form parameter `{}`", param.id);

            if let FormParamKind::File = param.kind {
                let id: ChainId = Identifier::escape(&param.id).into();
                let Some(path) = &param.file_name else {
                    error!(
                        "Form param `{}` is of type `file` \
                        but missing `file_name` field",
                        param.id
                    );
                    continue;
                };
                chains.insert(
                    id.clone(),
                    Chain {
                        id,
                        source: ChainSource::File {
                            path: Template::raw(path.to_owned()),
                        },
                        sensitive: false,
                        selector: None,
                        selector_mode: SelectorMode::default(),
                        content_type: None,
                        trim: Default::default(),
                    },
                );
            }
        }
    }

    chains
}

/// Expand the flat list of Insomnia resources into a recipe tree
fn build_recipe_tree(
    workspace_id: &str,
    request_groups: Vec<RequestGroup>,
    requests: Vec<Request>,
) -> anyhow::Result<RecipeTree> {
    /// Recursively build the recipe tree by removing children from the given
    /// map, starting with a particular parent node
    fn build_tree(
        children_map: &mut HashMap<String, Vec<RecipeNode>>,
        parent_id: &str,
    ) -> anyhow::Result<IndexMap<RecipeId, RecipeNode>> {
        // Pull in all the kids
        let children = children_map.remove(parent_id).ok_or_else(|| {
            anyhow!("No children found for parent `{parent_id}`")
        })?;
        let mut tree: IndexMap<RecipeId, RecipeNode> = children
            .into_iter()
            .map(|child| (child.id().clone(), child))
            .collect();

        // Recursively build out our family
        for child in tree.values_mut() {
            if let RecipeNode::Folder(folder) = child {
                folder.children = build_tree(children_map, &folder.id)?;
            }
        }

        Ok(tree)
    }

    // First, we want to match each parent with its children. Hashmap is fine
    // because we won't be iterating over it
    let mut children_map: HashMap<String, Vec<RecipeNode>> = request_groups
        .into_iter()
        .map(|request_group| {
            (
                request_group.parent_id.clone(),
                RecipeNode::from(request_group),
            )
        })
        .chain(requests.into_iter().map(|request| {
            (request.parent_id.clone(), RecipeNode::from(request))
        }))
        .into_group_map();

    let tree = build_tree(&mut children_map, workspace_id)?;
    Ok(RecipeTree::new(tree)?)
}

/// Deserialize a value using its `FromStr` implementation
fn deserialize_from_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    let s = String::deserialize(deserializer)?;
    s.parse().map_err(D::Error::custom)
}

/// For some fucked reason, Insomnia uses empty map instead of `null` for empty
/// values in some cases. This function deserializes that to a regular Option.
fn deserialize_shitty_option<'de, T, D>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    // Use a derived implementation from a wrapper struct. I tried to write this
    // myself, but couldn't figure it out. This means we get shitty errors :(

    #[derive(Deserialize)]
    #[serde(untagged, deny_unknown_fields)]
    enum MapOption<T> {
        None {},
        Some(T),
    }

    MapOption::<T>::deserialize(deserializer).map(|value| match value {
        MapOption::None {} => None,
        MapOption::Some(value) => Some(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde::de::DeserializeOwned;
    use serde_test::{Token, assert_de_tokens, assert_de_tokens_error};
    use slumber_util::test_data_dir;
    use std::{fmt::Debug, path::PathBuf};

    const INSOMNIA_FILE: &str = "insomnia.json";
    /// Assertion expectation is stored in a separate file. This is for a couple
    /// reasons:
    /// - It's huge so it makes code hard to navigate
    /// - Changes don't require a re-compile
    const INSOMNIA_IMPORTED_FILE: &str = "insomnia_imported.yml";

    /// Catch-all test for insomnia import
    #[rstest]
    #[tokio::test]
    async fn test_insomnia_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(INSOMNIA_FILE));
        let imported = from_insomnia(&input).await.unwrap();
        let expected =
            Collection::load(&test_data_dir.join(INSOMNIA_IMPORTED_FILE))
                .unwrap();
        assert_eq!(imported, expected);
    }

    #[test]
    fn test_deserialize_shitty_option() {
        /// A wrapper to use our custom deserializer
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(transparent)]
        struct Wrap<T: DeserializeOwned>(
            #[serde(deserialize_with = "super::deserialize_shitty_option")]
            Option<T>,
        );

        #[derive(Debug, PartialEq, Deserialize)]
        struct Test {
            a: String,
            b: i32,
        }

        assert_de_tokens(&Wrap(Some(3)), &[Token::I32(3)]);
        // Empty map with size hint
        assert_de_tokens(
            &Wrap::<i32>(None),
            &[Token::Map { len: Some(0) }, Token::MapEnd],
        );
        // Empty map without size hint
        assert_de_tokens(
            &Wrap::<i32>(None),
            &[Token::Map { len: None }, Token::MapEnd],
        );
        // Empty map without size hint
        assert_de_tokens(
            &Wrap::<i32>(None),
            &[Token::Map { len: None }, Token::MapEnd],
        );

        // Struct
        assert_de_tokens(
            &Wrap(Some(Test {
                a: "test".into(),
                b: 3,
            })),
            &[
                Token::Map { len: Some(4) },
                Token::Str("a"),
                Token::Str("test"),
                Token::Str("b"),
                Token::I32(3),
                Token::MapEnd,
            ],
        );
        // With size hint
        assert_de_tokens(
            &Wrap::<Test>(None),
            &[Token::Map { len: Some(0) }, Token::MapEnd],
        );
        // Without size hint
        assert_de_tokens(
            &Wrap::<Test>(None),
            &[Token::Map { len: None }, Token::MapEnd],
        );

        // Dynamic map
        assert_de_tokens(
            &Wrap(Some(indexmap! {
                6 => 36,
                3 => 9,
            })),
            &[
                Token::Map { len: Some(4) },
                Token::I32(6),
                Token::I32(36),
                Token::I32(3),
                Token::I32(9),
                Token::MapEnd,
            ],
        );
        // With size hint
        assert_de_tokens(
            &Wrap::<Test>(None),
            &[Token::Map { len: Some(0) }, Token::MapEnd],
        );
        // Without size hint
        assert_de_tokens(
            &Wrap::<IndexMap<i32, i32>>(None),
            &[Token::Map { len: None }, Token::MapEnd],
        );

        assert_de_tokens_error::<Wrap<Test>>(
            &[
                Token::Map { len: Some(4) },
                Token::Str("a"),
                Token::Str("test"),
                // Missing field
                Token::MapEnd,
            ],
            "data did not match any variant of untagged enum MapOption",
        );
    }
}
