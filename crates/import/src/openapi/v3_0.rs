//! OpenAPI v3.0 importer

use crate::{common, openapi::resolve::ReferenceResolver};
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use itertools::Itertools;
use mime::Mime;
use openapiv3::{
    APIKeyLocation, AnySchema, Components, MediaType, ObjectType, OpenAPI,
    Operation, Parameter, PathItem, PathStyle, Paths, ReferenceOr, RequestBody,
    Schema, SchemaKind, SecurityScheme, Server, Type,
};
use slumber_core::{
    collection::{
        Authentication, Collection, DuplicateRecipeIdError, Folder, Profile,
        ProfileId, Recipe, RecipeBody, RecipeId, RecipeNode, RecipeTree,
    },
    http::HttpMethod,
};
use slumber_template::Template;
use slumber_util::ResultTraced;
use std::iter;
use strum::IntoEnumIterator;
use tracing::{debug, error, warn};

/// Loads a collection from an OpenAPI v3.0 specification
pub fn from_openapi_v3_0(
    spec: serde_yaml::Value,
) -> anyhow::Result<Collection> {
    let OpenAPI {
        info,
        components,
        paths,
        servers,
        ..
    } = serde_yaml::from_value(spec)
        .context("Error deserializing OpenAPI collection")?;

    let profiles = build_profiles(servers);
    let recipes = build_recipe_tree(paths, components)?;

    Ok(Collection {
        name: Some(info.title),
        profiles,
        recipes,
    })
}

/// Build one profile per server
fn build_profiles(servers: Vec<Server>) -> IndexMap<ProfileId, Profile> {
    servers
        .into_iter()
        .map(|Server { url, variables, .. }| {
            let id: ProfileId = url.clone().into();
            // Include a "host" variable for each server, but allow the
            // user-defined variables to override that
            let data =
                iter::once(("host".to_owned(), Template::raw(url.clone())))
                    .chain(variables.into_iter().flatten().map(
                        |(name, variable)| {
                            (name, Template::raw(variable.default))
                        },
                    ))
                    .collect();
            (
                id.clone(),
                Profile {
                    id,
                    // We could just omit this and fall back to the ID which
                    // will be the same value, but we provide it for
                    // discoverability; the user may want to rename it
                    name: Some(url),
                    default: false,
                    data,
                },
            )
        })
        .collect()
}

/// Build a recipe tree out of all paths. Each path can have multiple operations
/// (up to one per method), and each operation becomes a recipe. We'll also
/// attempt to create folders from tags. Tags:operations are m2m but
/// folders:recipes are o2m, so we'll just take the first tag for operation.
///
/// The *only* way this can fail is if we get an ID collision in the recipe
/// tree. All other errors will be non-fatal.
fn build_recipe_tree(
    paths: Paths,
    components: Option<Components>,
) -> Result<RecipeTree, DuplicateRecipeIdError> {
    let reference_resolver = ReferenceResolver::new(components);
    let mut recipes: IndexMap<RecipeId, RecipeNode> = IndexMap::new();

    // Helper to add a recipe to the tree, and potentially a folder too
    let mut add_recipe = |path: &str, mut operation: Operation, method| {
        let first_tag = if operation.tags.is_empty() {
            None
        } else {
            Some(operation.tags.swap_remove(0))
        };
        let recipe = RecipeBuilder::build_recipe(
            operation,
            &reference_resolver,
            path,
            method,
        );

        let recipe_id = recipe.id.clone();
        let recipe_node = RecipeNode::Recipe(recipe);

        // If the recipe has any tags, insert into a folder. Otherwise insert
        // into the root
        if let Some(tag) = first_tag {
            let folder_id: RecipeId = format!("tag/{tag}").into();
            debug!("Inserting recipe `{recipe_id}` in folder `{folder_id}`");
            let node = recipes.entry(folder_id.clone()).or_insert_with(|| {
                Folder {
                    id: folder_id,
                    name: Some(tag),
                    children: IndexMap::default(),
                }
                .into()
            });

            // If a recipe already exists with the folder ID we generated,
            // that's a logic error and should be fatal
            match node {
                RecipeNode::Folder(folder) => {
                    folder.children.insert(recipe_id, recipe_node);
                }
                RecipeNode::Recipe(recipe) => {
                    // Skip the folder but retain the recipe
                    error!(
                        "Cannot create folder `{}`; \
                        a recipe already exists with that ID",
                        recipe.id
                    );
                    recipes.insert(recipe_id, recipe_node);
                }
            }
        } else {
            debug!("Inserting recipe `{recipe_id}`");
            recipes.insert(recipe_id, recipe_node);
        }
    };

    // Each path can have multiple methods. Each path:method pair maps to one
    // recipe
    for (path, item) in paths.paths {
        match item {
            ReferenceOr::Item(mut path_item) => {
                // Note: we may fuck up the ordering of operations here. The
                // ordering is already lost because the openapi lib deserializes
                // into static fields instead of a map, so there's nothing we
                // can do to preserve the old order
                for method in HttpMethod::iter() {
                    if let Some(op) = take_operation(&mut path_item, method) {
                        add_recipe(&path, op, method);
                    }
                }
            }
            ReferenceOr::Reference { reference } => {
                // According to the spec, only *external* references can be used
                // here, and we don't support those, so don't bother
                // https://spec.openapis.org/oas/v3.0.3#path-item-object
                error!("References not supported for path items `{reference}`");
            }
        }
    }

    // Error occurs if we have any duplicate folder/recipe IDs. This *shouldn't*
    // happen because the OpenAPI spec requires op IDs to be unique:
    // https://spec.openapis.org/oas/v3.0.3#fixed-fields-7
    // It's possible a recipe ID collides with a folder ID, but that's very
    // unlikely because we namespace all the folders under tag/
    RecipeTree::new(recipes)
}

/// Get an operation from a path corresponding to a specific method. This will
/// move the operation out of its wrapping Option if present in order to return
/// an owned value. The goal here is to prevent a clone.
fn take_operation(
    path_item: &mut PathItem,
    method: HttpMethod,
) -> Option<Operation> {
    match method {
        HttpMethod::Connect => None,
        HttpMethod::Delete => path_item.delete.take(),
        HttpMethod::Get => path_item.get.take(),
        HttpMethod::Head => path_item.head.take(),
        HttpMethod::Options => path_item.options.take(),
        HttpMethod::Patch => path_item.patch.take(),
        HttpMethod::Post => path_item.post.take(),
        HttpMethod::Put => path_item.put.take(),
        HttpMethod::Trace => path_item.trace.take(),
    }
}

/// Helper struct to hold intermediate state while converting an operation into
/// a recipe
struct RecipeBuilder<'a> {
    id: RecipeId,
    name: String,
    method: HttpMethod,
    url: String,
    body: Option<RecipeBody>,
    authentication: Option<Authentication>,
    query: Vec<(String, Template)>,
    headers: IndexMap<String, Template>,
    reference_resolver: &'a ReferenceResolver,
}

impl<'a> RecipeBuilder<'a> {
    /// Translate an OpenAPI Operation into a recipe
    fn build_recipe(
        operation: Operation,
        reference_resolver: &'a ReferenceResolver,
        path_name: &str,
        method: HttpMethod,
    ) -> Recipe {
        // Use operation_id if one is provided, otherwise generate one
        let id: RecipeId = operation
            .operation_id
            .unwrap_or_else(|| format!("{path_name}-{method}"))
            .into();
        let name = operation.summary.unwrap_or_else(|| path_name.to_owned());
        // Build the base URL template. We may modify this to replace its path
        // params with corresponding chain references, so don't convert it into
        // a template until the end
        let url = format!("{{{{ host }}}}{path_name}");

        let mut builder = Self {
            id,
            url,
            name,
            method,
            body: None,
            authentication: None,
            query: Vec::new(),
            headers: IndexMap::new(),
            reference_resolver,
        };

        if let Some(request_body) = operation.request_body {
            builder.process_body(request_body);
        }
        builder.process_parameters(operation.parameters);
        builder.process_security(operation.security);

        // Build the URL from the template we generated
        let url = builder
            .url
            .parse()
            // We just built this template ourselves, so hopefully parsing
            // doesn't fail. If it does, fall back to the original URL
            .with_context(|| {
                format!(
                "Error generating URL for recipe `{}`; plain path will be used",
                builder.id
            )
            })
            .traced()
            .unwrap_or_else(|_| Template::raw(path_name.to_owned()));

        Recipe {
            id: builder.id,
            persist: true,
            name: Some(builder.name),
            method: builder.method,
            url,
            body: builder.body,
            authentication: builder.authentication,
            query: common::build_query_parameters(builder.query),
            headers: builder.headers,
        }
    }

    /// Imperatively update the recipe to according to various parameters.
    /// OpenAPI parameters can map to query params, headers, path params, or
    /// cookies. We have to take the URL as a separate param because templates
    /// can't be modified. This prevents us from having to stringify and
    /// re-parse the template every time we make a modification.
    fn process_parameters(&mut self, parameters: Vec<ReferenceOr<Parameter>>) {
        parameters
            .into_iter()
            .filter_map(|parameter| {
                // For any reference that fails to resolve, print the error and
                // throw it away
                self.reference_resolver
                    .resolve::<Parameter>(
                        format!("{id}.parameters", id = self.id),
                        parameter,
                    )
                    .ok()
            })
            .for_each(|parameter| match parameter.into_owned() {
                Parameter::Query { parameter_data, .. } => {
                    self.query.push((parameter_data.name, Template::default()));
                }
                Parameter::Header { parameter_data, .. } => {
                    // if the name field is "Accept", "Content-Type" or
                    // "Authorization", the parameter definition SHALL be
                    // ignored. https://spec.openapis.org/oas/v3.0.3#fixed-fields-9
                    let name = parameter_data.name;
                    match name.as_str() {
                        "Accept" | "Content-Type" | "Authorization" => {}
                        _ => {
                            self.headers.insert(name, Template::default());
                        }
                    }
                }
                Parameter::Path {
                    style: PathStyle::Simple,
                    parameter_data,
                } => {
                    // Replace path params with a template key. The key probably
                    // won't refer to anything, but it's better than being
                    // completely invalid. We have no way of knowing how the
                    // user actually wants to fill this
                    // value
                    let id = parameter_data.name;
                    // {id} -> {{id}}
                    self.url = self.url.replace(
                        &format!("{{{id}}}"),
                        &format!("{{{{{id}}}}}"),
                    );
                }
                Parameter::Path {
                    style: PathStyle::Matrix | PathStyle::Label,
                    parameter_data,
                } => {
                    error!(
                        "Unsupported type for path param `{}`",
                        parameter_data.name
                    );
                }
                Parameter::Cookie { parameter_data, .. } => {
                    error!(
                        "Unsupported parameter type cookie for param `{}`",
                        parameter_data.name
                    );
                }
            });
    }

    /// Imperatively update the recipe to include security scheme(s). Depending
    /// on the scheme this may map to first-class auth, query params, or headers
    fn process_security(
        &mut self,
        security: Option<Vec<IndexMap<String, Vec<String>>>>,
    ) {
        security
            // Flatten Option<Vec<IndexMap<_>>> into just the keys, because we
            // don't care about the values for each scheme
            .unwrap_or_default()
            .into_iter()
            .flat_map(IndexMap::into_keys)
            // Resolve references, throwing away invalid ones
            .filter_map(|scheme_name| {
                self.reference_resolver
                    .get_by_name::<SecurityScheme>(&scheme_name)
                    .with_context(|| {
                        format!("{}.security.{scheme_name}", self.id,)
                    })
                    .traced()
                    .ok()
            })
            .for_each(|scheme| match scheme {
                // Where we need an auth value but don't have one, we'll create
                // a placeholder template. This should improve
                // discoverability for template features. We
                // don't expect the fields to actually map to
                // anything.
                SecurityScheme::HTTP {
                    scheme,
                    bearer_format,
                    ..
                } => match scheme.as_str() {
                    "Basic" | "basic" => {
                        self.authentication = Some(Authentication::Basic {
                            username: Template::from_field("username"),
                            password: Some(Template::from_field("password")),
                        });
                    }
                    "Bearer" | "bearer" => {
                        let template = bearer_format
                            .clone()
                            .map(Template::raw)
                            .unwrap_or_default();
                        self.authentication =
                            Some(Authentication::Bearer { token: template });
                    }
                    unsupported => {
                        error!(
                        "Unsupported HTTP Authentication scheme {unsupported}"
                    );
                    }
                },
                SecurityScheme::APIKey { location, name, .. } => match location
                {
                    APIKeyLocation::Query => self.query.push((
                        name.clone(),
                        Template::from_field("api_key"),
                    )),
                    APIKeyLocation::Header => {
                        self.headers.insert(
                            name.clone(),
                            Template::from_field("api_key"),
                        );
                    }
                    APIKeyLocation::Cookie => {
                        error!("Unsupported API key location: Cookies");
                    }
                },
                SecurityScheme::OAuth2 { .. } => {
                    error!("Unsupported security scheme: OAuth2");
                }
                SecurityScheme::OpenIDConnect { .. } => {
                    error!("Unsupported security scheme: OpenIDConnect");
                }
            });
    }

    /// Imperatively set the request body. The body can contain multiple
    /// examples, either on the operation itself or the underlying schema. We'll
    /// grab the first valid one.
    fn process_body(&mut self, request_body: ReferenceOr<RequestBody>) {
        // If the reference is invalid, log it and fuck off
        let Ok(request_body) = self.reference_resolver.resolve::<RequestBody>(
            format!("{}.requestBody", self.id),
            request_body,
        ) else {
            return;
        };

        let body = request_body
            .content
            .iter()
            // Parse each MIME type and exclude bodies with an invalid type
            .filter_map(|(content_type, media_type)| {
                let mime = content_type
                    .parse::<Mime>()
                    .map_err(|_| {
                        anyhow!(
                            "Invalid MIME type `{content_type}` for \
                            recipe `{}`",
                            self.id
                        )
                    })
                    .traced()
                    .ok()?;
                Some((mime, media_type))
            })
            // Each MIME type could have multiple examples. We want all of them
            // in case the first is invalid somehow
            .flat_map(|(mime, media_type)| {
                self.get_examples(mime.clone(), media_type)
                    .map(move |body| (mime.clone(), body))
            })
            // Convert each example into our body format
            .filter_map(|(mime, value)| {
                self.convert_body(&mime, value)
                    .with_context(|| format!("{}.requestBody.{mime}", self.id))
                    .traced()
                    .ok()
            })
            // Sort known content types first
            .sorted_by_key(|body| {
                // This means bodies that *don't* match will sort first, because
                // false < true
                matches!(body, RecipeBody::Raw(_))
            })
            .next();

        if let Some(body) = body {
            self.body = Some(body);
        } else {
            error!(
                "No bodies with supported content type for recipe `{}`",
                self.id
            );
        }
    }

    /// Get all examples for a particular media type. This combines these
    /// sources (in order of decreasing precedence):
    /// - `media_type.example`
    /// - `media_type.examples` (according to the spec this is mutually
    ///   exclusive with `media_type.example`, but we support both because it's
    ///   easy)
    /// - `media_type.schema.examples`
    /// - `media_type.schema.properties` (build an example from the schema def)
    fn get_examples(
        &'a self,
        mime: Mime,
        media_type: &'a MediaType,
    ) -> impl 'a + Iterator<Item = serde_json::Value> {
        // These are ordered by precedence. If any is empty we'll fall back to
        // the next one
        let example = media_type.example.clone();
        let mime_ = mime.clone();
        let examples =
            media_type
                .examples
                .iter()
                .filter_map(move |(name, example)| {
                    let example = self
                        .reference_resolver
                        .resolve(
                            format!(
                                "{id}.requestBody.content.{mime_}\
                                .examples.{name}",
                                id = self.id,
                            ),
                            example.clone(),
                        )
                        .ok()?
                        .into_owned();
                    example.value
                });

        // If there was no example for the operation, look at the underlying
        // schema
        let schema_example = media_type.schema.as_ref().and_then(|schema| {
            let schema_path = format!(
                "{id}.requestBody.content.{mime}.schema",
                id = self.id,
            );
            let schema = self
                .reference_resolver
                .resolve::<Schema>(schema_path.clone(), schema.clone())
                .ok()?;

            // If there's no example declared on the schema, generate one from
            // its properties
            schema
                .schema_data
                .example
                .clone()
                .or_else(|| Some(self.schema_to_json(&schema, schema_path)))
        });

        example.into_iter().chain(examples).chain(schema_example)
    }

    /// Build an example JSON value from a schema definition. This will be
    /// called recursively to convert individual parts of complex bodies
    fn schema_to_json(
        &self,
        schema: &Schema,
        schema_path: String,
    ) -> serde_json::Value {
        fn first<T>(enumeration: &[Option<T>]) -> Option<&T> {
            enumeration.first().and_then(Option::as_ref)
        }

        // If an example value exists, use it
        if let Some(example) = &schema.schema_data.example {
            return example.clone();
        }

        match &schema.schema_kind {
            // Any boolean enum is just going go be [false, true]
            SchemaKind::Type(Type::Boolean(_)) => false.into(),
            // Floats
            SchemaKind::Type(Type::Number(number)) => {
                // Try the first value in the enum
                first(&number.enumeration)
                    .copied()
                    // Then try minimum or maximum to ensure the value is valid
                    .or(number.minimum)
                    .or(number.maximum)
                    // Fallback to a default
                    .unwrap_or(0.0)
                    .into()
            }
            SchemaKind::Type(Type::Integer(integer)) => {
                // Try the first value in the enum
                first(&integer.enumeration)
                    .copied()
                    // Then try minimum or maximum to ensure the value is valid
                    .or(integer.minimum)
                    .or(integer.maximum)
                    // Fallback to a default
                    .unwrap_or(0)
                    .into()
            }
            SchemaKind::Type(Type::String(string)) => {
                // Try the first value in the enum
                first(&string.enumeration)
                    .map(String::from)
                    // Otherwise use the default. We could try to come up with
                    // something based on the `format` or `pattern` fields but
                    // I'm taking a shortcut
                    .unwrap_or_default()
                    .into()
            }

            SchemaKind::Type(Type::Array(array)) => {
                let vec = if let Some(schema) =
                    array.items.clone().and_then(|schema| {
                        self.reference_resolver
                            // This path is kinda bogus but tracking where in
                            // the object we actually are is a lot harder
                            .resolve(schema_path.clone(), schema)
                            .ok()
                    }) {
                    // Convert the inner schema to a value and wrap it
                    vec![self.schema_to_json(&schema, schema_path)]
                } else {
                    vec![]
                };
                serde_json::Value::Array(vec)
            }
            // For an object, expand all its properties. This is the primary
            // case for the top level of a structured body
            SchemaKind::Type(Type::Object(ObjectType {
                properties, ..
            }))
            | SchemaKind::Any(AnySchema { properties, .. }) => {
                let map = properties
                    .iter()
                    .filter_map(|(property, schema)| {
                        let schema = self
                            .reference_resolver
                            .resolve(schema_path.clone(), schema.clone())
                            .ok()?;
                        let value =
                            self.schema_to_json(&schema, schema_path.clone());
                        Some((property.clone(), value))
                    })
                    .collect();
                serde_json::Value::Object(map)
            }

            SchemaKind::OneOf { one_of: schemas }
            | SchemaKind::AllOf { all_of: schemas }
            | SchemaKind::AnyOf { any_of: schemas } => {
                // Grab the first schema in the list and use its body.
                // Technically for allOf we should join properties from all
                // fields but I'm being lazy
                let Some(schema) = schemas.first().and_then(|schema| {
                    self.reference_resolver
                        .resolve(schema_path.clone(), schema.clone())
                        .ok()
                }) else {
                    return serde_json::Value::Null;
                };
                self.schema_to_json(&schema, schema_path)
            }
            // We can't infer anything about what should go here
            SchemaKind::Not { .. } => serde_json::Value::Null,
        }
    }

    /// Convert a body of a single example to a recipe body
    fn convert_body(
        &self,
        mime: &Mime,
        body: serde_json::Value,
    ) -> anyhow::Result<RecipeBody> {
        fn unwrap_object(
            value: serde_json::Value,
        ) -> anyhow::Result<IndexMap<String, Template>> {
            // This may not be correct, but we'll just stringify the value as
            // JSON https://swagger.io/docs/specification/describing-request-body/multipart-requests/
            if let serde_json::Value::Object(object) = value {
                Ok(object
                    .into_iter()
                    .map(|(key, value)| {
                        // Convert value to string
                        let value = match value {
                            serde_json::Value::String(s) => s,
                            // Do *not* prettify here; we want to be able to
                            // show this in one line in the UI
                            _ => value.to_string(),
                        };
                        (key, Template::raw(value))
                    })
                    .collect())
            } else {
                Err(anyhow!("Expected object"))
            }
        }

        if mime == &mime::APPLICATION_JSON {
            // Currently we don't match against any JSON extensions. Just a
            // shortcut, could fix later
            Ok(RecipeBody::untemplated_json(body))
        } else if mime == &mime::APPLICATION_WWW_FORM_URLENCODED {
            let form = unwrap_object(body)?;
            Ok(RecipeBody::FormUrlencoded(form))
        } else if mime == &mime::MULTIPART_FORM_DATA {
            let form = unwrap_object(body)?;
            Ok(RecipeBody::FormMultipart(form))
        } else {
            warn!(
                "Unknown content type `{mime}` for body of recipe `{}`",
                self.id
            );
            Ok(RecipeBody::Raw(Template::raw(format!("{body:#}"))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use openapiv3::{Example, Schema, SchemaData, SchemaKind, Type};
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use serde_json::json;
    use std::sync::OnceLock;

    /// Test various cases of [RequestBuilder::process_body]
    #[rstest]
    #[case::json(
        [(
            "application/json",
            MediaType {
                example: Some(json!({"field": "value"})),
                ..Default::default()
            }
        )],
        RecipeBody::untemplated_json(json!({"field": "value"})),
    )]
    #[case::form_urlencoded(
        [(
            "application/x-www-form-urlencoded",
            MediaType {
                example: Some(json!({"field": "value", "complex": [1, 2]})),
                ..Default::default()
            }
        )],
        RecipeBody::FormUrlencoded(indexmap! {
            // Complex value gets stringified
            "complex".into() => "[1,2]".into(),
            "field".into() => "value".into(),
        }),
    )]
    #[case::form_multipart(
        [(
            "multipart/form-data",
            MediaType {
                example: Some(json!({"field": "value", "complex": [1, 2]})),
                ..Default::default()
            }
        )],
        RecipeBody::FormMultipart(indexmap! {
            // Complex value gets stringified
            "complex".into() => "[1,2]".into(),
            "field".into() => "value".into(),
        }),
    )]
    #[case::raw(
        [(
            "application/xml",
            MediaType {
                example: Some(json!({"field": "value"})),
                ..Default::default()
            }
        )],
        RecipeBody::Raw("{\n  \"field\": \"value\"\n}".into()),
    )]
    // We can load from the `schema.example` field
    #[case::schema_example(
        [(
            "application/json",
            MediaType {
                schema: Some(ReferenceOr::Item(Schema {
                    schema_data: SchemaData {
                        example: Some(json!({"field": "value"})),
                        ..Default::default()
                    },
                    schema_kind: SchemaKind::Type(Type::Object(Default::default())),
                })),
                ..Default::default()
            }
        )],
        RecipeBody::untemplated_json(json!({"field": "value"})),
    )]
    // We can load from the `examples` field
    #[case::examples_map(
        [(
            "application/json",
            MediaType {
                examples: indexmap! {
                    "example1".to_owned() => ReferenceOr::Item(Example {
                        value: Some(json!({"field": "value"})),
                        ..Default::default()
                    })
                },
                ..Default::default()
            }
        )],
        RecipeBody::untemplated_json(json!({"field": "value"})),
    )]
    // `example`` field takes priority over `schema.example`
    #[case::field_precedence(
        [(
            "application/json",
            MediaType {
                schema: Some(ReferenceOr::Item(Schema {
                    schema_data: SchemaData {
                        example: Some(json!({"field": "schema"})),
                        ..Default::default()
                    },
                    schema_kind: SchemaKind::Type(Type::Object(Default::default())),
                })),
                example: Some(json!({"field": "example"})),
                ..Default::default()
            }
        )],
        RecipeBody::untemplated_json(json!({"field": "example"})),
    )]
    // Known content type takes precedence over unknown ones
    #[case::content_type_precedence(
        [
            (
                "application/xml",
                MediaType {
                    example: Some(json!({"field": "value"})),
                    ..Default::default()
                }
            ),
            (
                "application/json",
                MediaType {
                    example: Some(json!({"field": "value"})),
                    ..Default::default()
                }
            ),
        ],
        RecipeBody::untemplated_json(json!({"field": "value"})),
    )]
    fn test_process_body(
        mut builder: RecipeBuilder<'static>,
        #[case] body_content: impl IntoIterator<Item = (&'static str, MediaType)>,
        #[case] expected: RecipeBody,
    ) {
        let request_body = RequestBody {
            description: None,
            content: body_content
                .into_iter()
                .map(|(content_type, media_type)| {
                    (content_type.to_owned(), media_type)
                })
                .collect(),
            required: false,
            extensions: Default::default(),
        };
        builder.process_body(ReferenceOr::Item(request_body));
        assert_eq!(builder.body.unwrap(), expected);
    }

    #[fixture]
    fn builder() -> RecipeBuilder<'static> {
        static RESOLVER: OnceLock<ReferenceResolver> = OnceLock::new();
        RecipeBuilder {
            id: "test".into(),
            name: "test".into(),
            method: HttpMethod::Get,
            url: "{{ host }}/get".into(),
            body: None,
            authentication: None,
            query: Default::default(),
            headers: Default::default(),
            reference_resolver: RESOLVER
                .get_or_init(|| ReferenceResolver::new(None)),
        }
    }
}
