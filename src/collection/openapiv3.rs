//! Import request collections from an OpenAPI v3.0.X specification.

use std::{fs::File, path::Path};

use crate::{
    collection::{Collection, RecipeId, RecipeNode, RecipeTree},
    template::Template,
};

use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use openapiv3::{OpenAPI, Operation, ReferenceOr};
use tracing::info;

use super::{Method, Recipe};

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
        let openapi: OpenAPI = serde_yaml::from_reader(file).context(
            format!("Error deserializing OpenAPIv3 collection file {openapiv3_specification_file:?}"),
        )?;

        let mut recipes = IndexMap::new();
        for (path_name, item) in openapi.paths.paths {
            let mut try_add_recipe_for_method =
                |maybe_operation: Option<Operation>, method: Method| -> anyhow::Result<()> {
                    if let Some(op) = maybe_operation {
                        let recipe =
                            operation_to_recipe(op, &path_name, method)?;
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
                    try_add_recipe_for_method(path_item.delete, Method::Delete)?;
                    try_add_recipe_for_method(path_item.options, Method::Options)?;
                    try_add_recipe_for_method(path_item.head, Method::Head)?;
                    try_add_recipe_for_method(path_item.trace, Method::Trace)?;
                }
                ReferenceOr::Reference { reference } => {
                    return Err(anyhow!("Unhandled reference to {reference}"));
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
fn operation_to_recipe(
    operation: Operation,
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
    let template= format!("{{{{host}}}}{path_name}");
    let url = Template::parse(template).context(format!("Failed to parse the template for recipe {id}"))?;

    // Name of the recipe
    let name = operation.summary.unwrap_or_else(|| path_name.clone());

    Ok(Recipe {
        id,
        name: Some(name),
        method,
        url,
        body: None,                   // TODO
        authentication: None,         // TODO
        query: IndexMap::default(),   // TODO
        headers: IndexMap::default(), // TODO
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
    const OPENAPIV3_IMPORTED_FILE: &str = "./test_data/openapiv3_petstore_imported.yml";

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
