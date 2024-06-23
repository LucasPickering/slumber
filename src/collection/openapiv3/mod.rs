//! Import request collections from an OpenAPI v3.0.X specification.
//!
//! # Usage
//!
//! The importer supports :
//! * profiles based on the servers defined in the provided OpenAPI specifications.
//! * recipes based on the paths defined in the provided OpenAPI specifications.
//!
//! OpenAPI operations can contain many tags. All recipes with the same tag will be grouped
//! into a folder, named after the tag.
//!
//! Note that only the first tag of an endpoint will be taken into consideration because Slumber
//! does not support having the same recipe in multiple folders.
//!
//! # Profiles
//!
//! Profiles are loaded based on the `servers` field at the root of the specifications,
//! where one `Server` will give one [`Profile`].
//!
//! Because the servers in an OpenAPI specification don't have an ID, the URL of the server
//! is used as the Profile's name for lack of a better default. The URL is also stored in
//! the data of the profile as a magic variable named `host`.
//!
//! All the variables defined in the Server instance are propagated to the data of the profile.
//!
//! # Recipes
//!
//! Recipes are loaded based on the `paths` field at the root of the specifications,
//! where each `path` can add one recipe for each kind of supported HTTP Method.
//!
//! The recipe's name is the operation's description.
//!
//! The Recipe will always try to use the `host` variable from the profile as the base of the URL,
//! and add the path after it.
//!
//! Query parameters and Header parameters are supported.
//!
//! Authentication is supported, though Slumber lacks support for some of the authority schemes
//! that are supported by OpenAPI v3.
//!
//! The body of the recipe is loaded from the example field of the FIRST content-type defined in
//! the specifications of the endpoint.
//!

mod resolve;

use std::{fs::File, path::Path};

use crate::{
    collection::{
        openapiv3::resolve::OpenApiReferenceResolver, Authentication,
        Collection, Folder, Method, Profile, ProfileId, Recipe, RecipeId,
        RecipeNode, RecipeTree,
    },
    template::Template,
};

use anyhow::{anyhow, Context};
use indexmap::{map::Entry, IndexMap};
use openapiv3::{
    APIKeyLocation, OpenAPI, Operation, Parameter, ReferenceOr, SecurityScheme,
    Server,
};
use tracing::{info, warn};

impl Collection {
    /// Loads a collection from an OpenAPIv3 specification file
    pub fn from_openapiv3(
        openapiv3_file: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let openapiv3_specification_file = openapiv3_file.as_ref();
        info!(file = ?openapiv3_specification_file, "Loading OpenAPI v3 (JSON) collection");

        let file = File::open(openapiv3_specification_file).context(format!(
            "Error opening OpenAPI v3 (JSON) collection file {openapiv3_specification_file:?}"
        ))?;

        // The format can be YAML or JSON, so we can just treat it all as YAML
        let OpenAPI {
            components,
            paths,
            servers,
            ..
        } = serde_yaml::from_reader(file).context(
            format!("Error deserializing OpenAPIv3 collection file {openapiv3_specification_file:?}"),
        )?;
        let reference_resolver = OpenApiReferenceResolver::new(components);

        let mut recipes = IndexMap::new();
        let mut tag_folders: IndexMap<String, Folder> = IndexMap::default();

        // Load Recipes, built by OpenAPI Operations
        for (path_name, item) in paths.paths {
            let mut try_add_recipe_for_method =
                |maybe_operation: Option<Operation>,
                 method: Method|
                 -> anyhow::Result<()> {
                    if let Some(op) = maybe_operation {
                        let tags = op.tags.clone();
                        let recipe = operation_to_recipe(
                            op,
                            &reference_resolver,
                            &path_name,
                            method,
                        )?;
                        let recipe_id = recipe.id.clone();
                        let recipe_node = RecipeNode::Recipe(recipe);
                        // OpenAPI supports using tags for your endpoints. Slumber will group
                        // recipes with the same tags in one folders.
                        //
                        // We do not support having the same recipe in multiple folders though,
                        // because duplicating recipes would make the slumber config harder to
                        // maintain. Let's grab the first tag instead, and that will be our folder
                        if let Some(tag) = tags.first() {
                            info!("Inserting the recipe {recipe_id} in folder {tag}");
                            match tag_folders.entry(tag.to_string()) {
                                Entry::Vacant(entry) => {
                                    let mut children = IndexMap::default();
                                    children.insert(recipe_id, recipe_node);
                                    entry.insert(Folder {
                                        id: RecipeId::from(format!(
                                            "tag/{tag}"
                                        )),
                                        name: Some(tag.clone()),
                                        children,
                                    });
                                }
                                Entry::Occupied(mut folder) => {
                                    folder
                                        .get_mut()
                                        .children
                                        .insert(recipe_id, recipe_node);
                                }
                            }
                        } else {
                            info!("Inserting the recipe {recipe_id}");
                            recipes.insert(recipe_id, recipe_node);
                        }
                    }
                    Ok(())
                };
            match item {
                ReferenceOr::Item(path_item) => {
                    try_add_recipe_for_method(path_item.get, Method::Get)?;
                    try_add_recipe_for_method(path_item.post, Method::Post)?;
                    try_add_recipe_for_method(path_item.put, Method::Put)?;
                    try_add_recipe_for_method(path_item.patch, Method::Patch)?;
                    try_add_recipe_for_method(
                        path_item.delete,
                        Method::Delete,
                    )?;
                    try_add_recipe_for_method(
                        path_item.options,
                        Method::Options,
                    )?;
                    try_add_recipe_for_method(path_item.head, Method::Head)?;
                    try_add_recipe_for_method(path_item.trace, Method::Trace)?;
                }
                ReferenceOr::Reference { reference } => {
                    return Err(anyhow!(
                        "Could not resolve reference to {reference}"
                    ));
                }
            }
        }
        tag_folders
            .into_values()
            .filter(|folder| !folder.children.is_empty())
            .for_each(|folder| {
                recipes.insert(folder.id.clone(), RecipeNode::Folder(folder));
            });
        let recipes =
            RecipeTree::new(recipes).map_err(|duplicated_recipe_id| {
                anyhow!("Duplicated Recipe ID: {duplicated_recipe_id}")
            })?;

        // Load profiles
        let mut profiles = IndexMap::default();
        for server in servers {
            let Server {
                url,
                variables,
                description: _,
                extensions: _,
            } = server;
            let mut data = IndexMap::default();
            if let Some(variables) = variables {
                for (var_name, variable) in variables {
                    let value = variable.default;
                    let variable =
                        Template::try_from(value.clone()).context(format!(
                            "Failed to parse variable {value} as a template"
                        ))?;
                    data.insert(var_name, variable);
                }
            }
            let host = Template::try_from(url.clone())
                .context(format!("Failed to parse URL {url} as a template"))?;
            data.insert("host".to_string(), host);
            let profile_id = ProfileId::from(format!("profile-{url}"));
            profiles.insert(
                profile_id.clone(),
                Profile {
                    id: profile_id,
                    name: Some(url),
                    data,
                },
            );
        }

        Ok(Collection {
            profiles,
            recipes,
            chains: IndexMap::new(),
            _ignore: serde::de::IgnoredAny,
        })
    }
}

/// Translates an OpenAPI Operation into a `Recipe` given the recipe's context
fn operation_to_recipe(
    operation: Operation,
    reference_resolver: &OpenApiReferenceResolver,
    path_name: &String,
    method: Method,
) -> anyhow::Result<Recipe> {
    // ID for the operation
    // Use operation_id if one is provided, otherwise generate a unique
    let id = match operation.operation_id {
        Some(id) => RecipeId::from(id),
        None => RecipeId::from(format!("{method} {path_name}")),
    };

    // URL
    let template = format!("{{{{host}}}}{path_name}");
    let url = Template::parse(template)
        .context(format!("Failed to parse the template for recipe {id}"))?;

    // Name of the recipe
    let name = operation.summary.unwrap_or_else(|| path_name.clone());

    // Parameters
    let mut query_params = IndexMap::default();
    let mut headers_params = IndexMap::default();
    for ref_param in operation.parameters {
        let param = match ref_param {
            ReferenceOr::Item(item) => Ok(item),
            ReferenceOr::Reference { reference } => reference_resolver
                .get_parameter(&reference)
                .context("Failed to resolve reference to Parameter")
                .cloned(),
        }?;
        // The following is quoted directly from the specifications of parameter objets
        // see: https://spec.openapis.org/oas/v3.0.3#parameter-object
        match param {
            Parameter::Query { parameter_data, .. } => {
                // the name corresponds to the parameter name used by the in property.
                query_params.insert(parameter_data.name, Template::empty());
            }
            Parameter::Header { parameter_data, .. } => {
                // if the name field is "Accept", "Content-Type" or "Authorization", the parameter definition SHALL be ignored.
                match parameter_data.name {
                    x if ["Accept", "Content-Type", "Authorization"]
                        .contains(&x.as_str()) =>
                    {
                        continue;
                    }
                    header => {
                        headers_params
                            .insert(header.to_string(), Template::empty());
                    }
                }
            }
            // TODO(path_parameters): Slumber does not support Path parameters
            Parameter::Path { .. } => {
                warn!("Unsupported parameter type: Path");
            }
            // TODO(cookies): Slumber does not support Cookie parameters
            Parameter::Cookie { .. } => {
                warn!("Unsupported parameter type: Cookie");
            }
        }
    }

    let mut http_auth = None;
    if let Some(security) = operation.security {
        for scheme in security {
            // From the specifications : https://spec.openapis.org/oas/v3.0.3#patterned-fields-2
            // If the security scheme is of type "oauth2" or "openIdConnect", then the value
            // is a list of scope names required for the execution, and the list MAY be empty
            // if authorization does not require a specified scope. For other security scheme
            // types, the array MUST be empty.
            for (name, values) in scheme {
                let security_scheme = reference_resolver
                    .get_security_scheme(&name)
                    .context("Failed to resolve the security scheme")?
                    .clone();
                match security_scheme {
                    SecurityScheme::HTTP {
                        scheme,
                        bearer_format,
                        ..
                    } => {
                        // Sanity-check spec complicance
                        if !values.is_empty() {
                            return Err(anyhow!("Spec error: For Security Scheme HTTP, values MUST be empty"));
                        }
                        match scheme.as_str() {
                            "Basic" | "basic" => {
                                http_auth = Some(Authentication::Basic {
                                    password: None,
                                    username: Template::empty(),
                                });
                            }
                            "Bearer" | "bearer" => {
                                let template = match bearer_format {
                                    Some(format) => Template::parse(format)
                                        .context("Failed to parse template")?,
                                    None => Template::empty(),
                                };
                                http_auth =
                                    Some(Authentication::Bearer(template));
                            }
                            unsupported => {
                                warn!("Unsupported HTTP Authentication scheme {unsupported}");
                            }
                        }
                    }
                    SecurityScheme::APIKey { location, name, .. } => {
                        // Sanity-check spec complicance
                        if !values.is_empty() {
                            return Err(anyhow!("Spec error: For Security Scheme APIKey, values MUST be empty"));
                        }
                        match location {
                            APIKeyLocation::Query => {
                                query_params.insert(name, Template::empty());
                            }
                            APIKeyLocation::Header => {
                                headers_params.insert(name, Template::empty());
                            }
                            // TODO(cookies): Slumber does not support Cookies
                            APIKeyLocation::Cookie => {
                                warn!("Unsupported APIKey Location: Cookies");
                            }
                        }
                    }
                    // TODO(oauth2): Slumber does not support OAuth2
                    SecurityScheme::OAuth2 { .. } => {
                        warn!("Unsupported Security Scheme OAuth2");
                    }
                    // TODO(openid): Slumber does not support OpenIDConnect
                    SecurityScheme::OpenIDConnect { .. } => {
                        warn!("Unsupported Security Scheme OpenIDConnect");
                    }
                }
            }
        }
    }

    let mut body = None;
    if let Some(request_body) = operation.request_body {
        let request_body = match request_body {
            ReferenceOr::Item(body) => Ok(body),
            ReferenceOr::Reference { reference } => reference_resolver
                .get_request_body(&reference)
                .context("Failed to resolve RequestBody reference")
                .cloned(),
        }?;
        // We don't support multiple body types, so let's just grab the first.
        if let Some((content_type, media_type)) = request_body.content.first() {
            if let Some(example) = &media_type.example {
                body = maybe_extract_body(content_type, example)
                    .context("Failed to extract body")?;
            }
        }
    }

    Ok(Recipe {
        id,
        name: Some(name),
        method,
        url,
        body,
        authentication: http_auth,
        query: query_params,
        headers: headers_params,
    })
}

fn maybe_extract_body(
    content_type: &str,
    media_type: &serde_json::Value,
) -> Result<Option<Template>, anyhow::Error> {
    match content_type {
        "application/json" => {
            let json_serialized = serde_json::to_string_pretty(media_type)
                .context("Failed to serialize body")?;
            let template = Template::try_from(json_serialized)
                .context("Failed to parse template")?;
            Ok(Some(template))
        }
        content_ty => {
            warn!("Unsupported content type {content_ty}");
            Ok(None)
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::collection::{Collection, CollectionFile};

    const OPENAPIV3_FILE: &str = "./test_data/openapiv3_petstore.yml";
    /// Assertion expectation is stored in a separate file. This is for a couple
    /// reasons:
    /// - It's huge so it makes code hard to navigate
    /// - Changes don't require a re-compile
    const OPENAPIV3_IMPORTED_FILE: &str =
        "./test_data/openapiv3_petstore_imported.yml";

    /// Catch-all test for openapiv3 import
    #[tokio::test]
    async fn test_openapiv3_import() {
        let imported = Collection::from_openapiv3(OPENAPIV3_FILE).unwrap();
        dbg!(&imported);
        let expected = CollectionFile::load(OPENAPIV3_IMPORTED_FILE.into())
            .await
            .unwrap()
            .collection;
        dbg!(&expected);
        assert_eq!(imported, expected);
    }
}
