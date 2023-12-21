//! Import request collections from Insomnia. Based on the Insomnia v4 export
//! format

use crate::{
    collection::{Collection, Profile, Recipe},
    template::Template,
};
use anyhow::Context;
use indexmap::IndexMap;
use serde::Deserialize;
use std::{fs::File, path::Path};
use tracing::info;

impl Collection {
    /// Convert an Insomnia exported collection into the slumber format. This
    /// supports YAML *or* JSON input.
    ///
    /// This is not async because it's only called by the CLI, where we don't
    /// care about blocking. It keeps the code simpler.
    pub fn from_insomnia(insomnia_file: &Path) -> anyhow::Result<Self> {
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
        let insomnia: Insomnia =
            serde_yaml::from_reader(file).context(format!(
                "Error deserializing Insomnia collection file {insomnia_file:?}"
            ))?;

        // Convert everything
        let mut profiles = IndexMap::new();
        let mut recipes = IndexMap::new();
        for resource in insomnia.resources {
            match resource {
                Resource::Request(request) => {
                    let request: super::Recipe = request.into();
                    recipes.insert(request.id.clone(), request);
                }
                Resource::Environment(environment) => {
                    let profile: super::Profile = environment.into();
                    profiles.insert(profile.id.clone(), profile);
                }
                // Everything else is TRASH
                _ => {}
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
    /// Maps to a recipe
    Request(Request),
    /// Maps to a profile
    Environment(Environment),
    // We don't use these
    RequestGroup,
    Workspace,
    CookieJar,
    ApiSpec,
}

#[derive(Debug, Deserialize)]
struct Environment {
    #[serde(rename = "_id")]
    id: String,
    name: String,
    data: IndexMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct Request {
    #[serde(rename = "_id")]
    id: String,
    name: String,
    url: Template,
    method: String,
    authentication: Authentication,
    headers: Vec<Header>,
    parameters: Vec<Parameter>,
    body: Body,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Authentication {
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

/// This can't be an `Option` because the empty case is an empty object, not
/// null
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Body {
    // This has to go *first*, otherwise all objects will match the empty case
    #[serde(rename_all = "camelCase")]
    Body {
        mime_type: String,
        text: Template,
    },
    // This matches empty object, so it has to be a struct variant
    Empty {},
}

impl From<Environment> for Profile {
    fn from(environment: Environment) -> Self {
        Profile {
            id: environment.id.into(),
            name: Some(environment.name),
            data: environment
                .data
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

impl From<Request> for Recipe {
    fn from(request: Request) -> Self {
        let mut headers: IndexMap<String, Template> = IndexMap::new();

        // Preload headers from implicit sources
        if let Body::Body { mime_type, .. } = &request.body {
            headers.insert(
                "content-type".into(),
                Template::dangerous_new(mime_type.clone()),
            );
        }
        match request.authentication {
            Authentication::Bearer { token } => {
                headers.insert(
                    "authorization".into(),
                    Template::dangerous_new(format!("Bearer {token}")),
                );
            }
        }
        // Load explicit headers *after* so we can override the implicit stuff
        for header in request.headers {
            headers.insert(header.name, header.value);
        }

        Recipe {
            id: request.id.into(),
            name: Some(request.name),
            method: request.method,
            url: request.url,
            body: match request.body {
                Body::Empty {} => None,
                Body::Body { text, .. } => Some(text),
            },
            query: request
                .parameters
                .into_iter()
                .map(|parameter| (parameter.name, parameter.value))
                .collect(),
            headers,
        }
    }
}
