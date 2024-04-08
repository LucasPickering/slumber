//! Import request collections from Insomnia. Based on the Insomnia v4 export
//! format

use crate::{
    collection::{
        self, Collection, Folder, Profile, Recipe, RecipeId, RecipeNode,
        RecipeTree,
    },
    template::Template,
};
use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use reqwest::header;
use serde::Deserialize;
use std::{collections::HashMap, fs::File, path::Path};
use tracing::info;

impl Collection {
    /// Convert an Insomnia exported collection into the slumber format. This
    /// supports YAML *or* JSON input.
    ///
    /// This is not async because it's only called by the CLI, where we don't
    /// care about blocking. It keeps the code simpler.
    pub fn from_insomnia(
        insomnia_file: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let insomnia_file = insomnia_file.as_ref();
        // First, deserialize into the insomnia format
        info!(file = ?insomnia_file, "Loading Insomnia collection");
        eprintln!(
            "WARNING: The Insomnia importer is *experimental*. It will \
            *not* give you an equivalent collection, and may not even work. It \
            is meant to give you a skeleton for a Slumber collection, and \
            nothing more."
        );
        let file = File::open(insomnia_file).context(format!(
            "Error opening Insomnia collection file {insomnia_file:?}"
        ))?;
        // The format can be YAML or JSON, so we can just treat it all as YAML
        let mut insomnia: Insomnia =
            serde_yaml::from_reader(file).context(format!(
                "Error deserializing Insomnia collection file {insomnia_file:?}"
            ))?;

        // Match Insomnia's visual order. This isn't entirely accurate because
        // Insomnia reorders folders/requests according to the tree structure,
        // but it should get us the right order within each layer
        insomnia.resources.sort_by_key(Resource::sort_key);

        // This will remove all the folders/requests from the list
        let recipes = build_recipe_tree(&mut insomnia.resources)?;

        // Convert everything left behind
        let mut profiles = IndexMap::new();
        for resource in insomnia.resources {
            if let Resource::Environment(environment) = resource {
                let profile: Profile = environment.into();
                profiles.insert(profile.id.clone(), profile);
            }
        }

        Ok(Collection {
            profiles,
            recipes,
            chains: IndexMap::new(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct Insomnia {
    resources: Vec<Resource>,
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
    CookieJar,
    ApiSpec,
}

/// A shitty option type. Insomnia uses empty map instead of `null` for empty
/// values in some cases. This type makes that easy to deserialize.
#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Opshit<T> {
    None {},
    Some(T),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Environment {
    #[serde(rename = "_id")]
    id: String,
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
    url: Template,
    method: String,
    authentication: Opshit<Authentication>,
    headers: Vec<Header>,
    parameters: Vec<Parameter>,
    body: Opshit<Body>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Authentication {
    Basic { username: String, password: String },
    Bearer { token: String },
    // Punting on other types for now
}

#[derive(Debug, Deserialize)]
struct Header {
    name: String,
    value: Template,
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    value: Template,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Body {
    mime_type: String,
    text: Template,
}

impl Resource {
    /// Rather than order things how they should be, Insomnia attaches a sort
    /// key to each item
    fn sort_key(&self) -> i64 {
        match self {
            Resource::RequestGroup(folder) => folder.meta_sort_key,
            Resource::Request(request) => request.meta_sort_key,
            Resource::Environment(environment) => environment.meta_sort_key,
            Resource::Workspace { .. } => 0,
            Resource::CookieJar => 0,
            Resource::ApiSpec => 0,
        }
    }
}

impl From<Environment> for Profile {
    fn from(environment: Environment) -> Self {
        Profile {
            id: environment.id.into(),
            name: Some(environment.name),
            data: environment
                .data
                .into_iter()
                .map(|(k, v)| (k, Template::dangerous(v)))
                .collect(),
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
        if let Opshit::Some(Body { mime_type, .. }) = &request.body {
            headers.insert(
                header::CONTENT_TYPE.as_str().into(),
                Template::dangerous(mime_type.clone()),
            );
        }
        // Load explicit headers *after* so we can override the implicit stuff
        for header in request.headers {
            headers.insert(header.name.to_lowercase(), header.value);
        }
        headers.shift_remove(header::USER_AGENT.as_str());

        // Load authentication scheme
        let authentication = match request.authentication {
            Opshit::None {} => None,
            Opshit::Some(Authentication::Basic { username, password }) => {
                Some(collection::Authentication::Basic {
                    username: Template::dangerous(username),
                    password: Some(Template::dangerous(password)),
                })
            }
            Opshit::Some(Authentication::Bearer { token }) => Some(
                collection::Authentication::Bearer(Template::dangerous(token)),
            ),
        };

        RecipeNode::Recipe(Recipe {
            id: request.id.into(),
            name: Some(request.name),
            method: request.method,
            url: request.url,
            body: match request.body {
                Opshit::None {} => None,
                Opshit::Some(Body { text, .. }) => Some(text),
            },
            query: request
                .parameters
                .into_iter()
                .map(|parameter| (parameter.name, parameter.value))
                .collect(),
            headers,
            authentication,
        })
    }
}

/// Expand the flat list of Insomnia resources into a recipe tree. This will
/// remove all folders and requests (AKA recipes) from the given vec, but leave
/// all other resources in place.
fn build_recipe_tree(
    resources: &mut Vec<Resource>,
) -> anyhow::Result<RecipeTree> {
    // First, we want to match each parent with its children
    let mut workspace_id: Option<String> = None;
    let mut children_map: HashMap<String, Vec<RecipeNode>> = HashMap::new();

    // We're going to drain the list of resources, then put the ones we don't
    // care about in a new list and leave that behind for our caller to handle
    let mut remaining: Vec<Resource> = Vec::new();
    for resource in resources.drain(..) {
        // We need to save the workspace ID because it's the root node
        if let Resource::Workspace { id } = &resource {
            workspace_id = Some(id.clone());
        }

        let (parent_id, node) = match resource {
            // Do conversion to Slumber types here, so we can enforce the
            // invariant that everything left is a folder or recipe
            Resource::RequestGroup(folder) => {
                (folder.parent_id.clone(), folder.into())
            }
            Resource::Request(request) => {
                (request.parent_id.clone(), request.into())
            }
            // Everything else is TRASH
            _ => {
                remaining.push(resource);
                continue;
            }
        };
        children_map.entry(parent_id).or_default().push(node);
    }
    *resources = remaining;

    // The workspace is the root node, so if we didn't find it we're hosed
    let workspace_id =
        workspace_id.ok_or_else(|| anyhow!("Workspace resource not found"))?;

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
                folder.children = build_tree(children_map, folder.id.as_str())?;
            }
        }

        Ok(tree)
    }

    let tree = build_tree(&mut children_map, &workspace_id)?;

    RecipeTree::new(tree).map_err(|duplicate_id| {
        anyhow!("Duplicate folder/recipe ID `{duplicate_id}`")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::CollectionFile;
    use pretty_assertions::assert_eq;

    const INSOMNIA_FILE: &str = "./test_data/insomnia.json";
    const INSOMNIA_IMPORTED_FILE: &str = "./test_data/insomnia_imported.yml";

    /// Catch-all test for insomnia import
    #[tokio::test]
    async fn test_insomnia_import() {
        let imported = Collection::from_insomnia(INSOMNIA_FILE).unwrap();
        let expected = CollectionFile::load(INSOMNIA_IMPORTED_FILE.into())
            .await
            .unwrap()
            .collection;
        assert_eq!(imported, expected);
    }
}
