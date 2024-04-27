//! Import request collections from an OpenAPI v3.0.X specification.

use std::{fs::File, path::Path};

use crate::{
    collection::{
        Collection, Method, Recipe, RecipeId, RecipeNode, RecipeTree,
        Authentication,
    },
    template::Template,
};

use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use openapiv3::{
    APIKeyLocation, OpenAPI, Operation, Parameter, ReferenceOr, SecurityScheme,
};
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
enum OpenAPIResolveError {
    #[error("The given OpenAPIv3 specs do not contain the `components` field")]
    MissingComponentsObject,
    #[error("Could not find the security scheme {_0} inside components.security_schemes")]
    SecuritySchemeNotFound(String),
    #[error("Could not resolve the reference {_0}")]
    UnhandledReference(String),
}

impl Collection {
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
            ..
        } = serde_yaml::from_reader(file).context(
            format!("Error deserializing OpenAPIv3 collection file {openapiv3_specification_file:?}"),
        )?;

        let resolve_security_scheme = move |scheme_name: String| -> Result<
            SecurityScheme,
            OpenAPIResolveError,
        > {
            let components = components
                .as_ref()
                .ok_or(OpenAPIResolveError::MissingComponentsObject)?;
            let ref_or_component = components
                .security_schemes
                .get(&scheme_name)
                .ok_or_else(|| {
                    OpenAPIResolveError::SecuritySchemeNotFound(scheme_name.clone())
                })?;
            match ref_or_component {
                ReferenceOr::Item(item) => Ok(item.clone()),
                ReferenceOr::Reference { reference: _ } => {
                    Err(OpenAPIResolveError::UnhandledReference(scheme_name))
                }
            }
        };
        let mut recipes = IndexMap::new();
        for (path_name, item) in paths.paths {
            let mut try_add_recipe_for_method =
                |maybe_operation: Option<Operation>,
                 method: Method|
                 -> anyhow::Result<()> {
                    if let Some(op) = maybe_operation {
                        let recipe = operation_to_recipe(
                            op,
                            &resolve_security_scheme,
                            &path_name,
                            method,
                        )?;
                        recipes.insert(
                            recipe.id.clone(),
                            RecipeNode::Recipe(recipe),
                        );
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

        let recipes =
            RecipeTree::new(recipes).map_err(|duplicated_recipe_id| {
                anyhow!("Duplicated Recipe ID: {duplicated_recipe_id}")
            })?;

        Ok(Collection {
            profiles: IndexMap::new(),
            recipes,
            chains: IndexMap::new(),
            _ignore: serde::de::IgnoredAny,
        })
    }
}

/// Translates an OpenAPI Operation into a `Recipe` given the recipe's context
fn operation_to_recipe<
    FSS: Fn(String) -> Result<SecurityScheme, OpenAPIResolveError>,
>(
    operation: Operation,
    resolve_security_schema: &FSS,
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

    let mut query_params = IndexMap::default();
    let mut headers_params = IndexMap::default();
    for ref_param in operation.parameters {
        let param = match ref_param {
            ReferenceOr::Item(item) => Ok(item),
            ReferenceOr::Reference { reference } => {
                // TODO: Resolve parameter
                Err(anyhow!("Could not resolve reference {reference}"))
            }
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
                        headers_params.insert(header.to_string(), Template::empty());
                    }
                }
            }
            // TODO: Support Path parameters
            Parameter::Path { .. } => {
                warn!("Unsupported parameter type: Path");
            },
            // TODO: Support Cookie parameters
            Parameter::Cookie { .. } => {
                warn!("Unsupported parameter type: Cookie");
            },
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
                let security_scheme = resolve_security_schema(name)
                    .context("Failed to resolve the security scheme")?;
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
                            },
                            "Bearer" | "bearer" => {
                                let template = match bearer_format {
                                    Some(format) => Template::parse(format).context("Failed to parse template")?,
                                    None => Template::empty(),
                                };
                                http_auth = Some(Authentication::Bearer(template));
                            },
                            unsupported => {
                                warn!("Unsupported HTTP Authentication scheme {unsupported}");
                            },
                        }
                    }
                    SecurityScheme::APIKey { location, name, .. } => {
                        // Sanity-check spec complicance
                        if !values.is_empty() {
                            return Err(anyhow!("Spec error: For Security Scheme APIKey, values MUST be empty"));
                        }
                        match location {
                            APIKeyLocation::Query => {
                                query_params.insert(
                                    name,
                                    Template::empty(),
                                );
                            }
                            APIKeyLocation::Header => {
                                headers_params.insert(
                                    name,
                                    Template::empty(),
                                );
                            }
                            // TODO: Support Cookies
                            APIKeyLocation::Cookie => {
                                warn!("Unsupported APIKey Location: Cookies");
                            },
                        }
                    }
                    // TODO: Support OAuth2
                    SecurityScheme::OAuth2 { .. } => {
                        warn!("Unsupported Security Scheme OAuth2");
                    },
                    // TODO: Support OpenIDConnect
                    SecurityScheme::OpenIDConnect { .. } => {
                        warn!("Unsupported Security Scheme OAuth2");
                    },
                }
            }
        }
    }

    Ok(Recipe {
        id,
        name: Some(name),
        method,
        url,
        body: None, // TODO
        authentication: http_auth,
        query: query_params,
        headers: headers_params,
    })
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
