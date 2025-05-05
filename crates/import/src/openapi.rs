//! Import request collections from an OpenAPI v3.0.X specification.
//!
//! - Servers are mapped to profiles
//!     - URL of the server is stored in the `host` field
//! - Operations (i.e. path-method pairs) are mapped to recipes
//! - Tags are mapped to folders
//!     - Since tags are m2m but folders are o2m, we only take the first tag
//! - References are resolved within the same file. We don't support resolving
//!   from other files.

mod resolve;

use crate::{
    ImportCollection,
    common::{Json, build_query_parameters, profile_field},
    openapi::resolve::ReferenceResolver,
};
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use itertools::Itertools;
use mime::Mime;
use openapiv3::{
    APIKeyLocation, Components, MediaType, OpenAPI, Operation, Parameter,
    PathItem, PathStyle, Paths, ReferenceOr, RequestBody, Schema,
    SecurityScheme, Server,
};
use petitscript::ast::{
    Expression, IntoExpression, TemplateChunk, TemplateLiteral,
};
use slumber_core::{
    collection::{
        Authentication, DuplicateRecipeIdError, Folder, Profile, ProfileId,
        Recipe, RecipeBody, RecipeId, RecipeNode, RecipeTree,
    },
    http::HttpMethod,
};
use slumber_util::{NEW_ISSUE_LINK, ResultTraced};
use std::{fs::File, iter, path::Path};
use strum::IntoEnumIterator;
use tracing::{debug, error, info, warn};

// TODO fix body import

/// Load a collection from an OpenAPI v3 specification file
pub fn from_openapi(
    openapi_file: impl AsRef<Path>,
) -> anyhow::Result<ImportCollection> {
    let path = openapi_file.as_ref();
    info!(file = ?path, "Loading OpenAPI collection");
    warn!(
        "The OpenAPI importer is approximate. Some features are missing \
            and it may not give you an equivalent or fulling functional
            collection. If you encounter a bug or would like to request support
            for a particular OpenAPI feature, please open an issue:
            {NEW_ISSUE_LINK}"
    );

    let file = File::open(path)
        .context(format!("Error opening OpenAPI collection file {path:?}"))?;

    // The format can be YAML or JSON, so we can just treat it all as YAML
    let OpenAPI {
        openapi: openapi_version,
        components,
        paths,
        servers,
        ..
    } = serde_yaml::from_reader(file).context(format!(
        "Error deserializing OpenAPI collection file {path:?}"
    ))?;

    if !openapi_version.starts_with("3.0.") {
        warn!(
            "Importer currently only supports OpenAPI v3.0, this spec is
                version {openapi_version}. We'll try the import anyway, but you
                may experience issues."
        )
    }

    let profiles = build_profiles(servers);
    let recipes = build_recipe_tree(paths, components)?;

    Ok(ImportCollection {
        declarations: Vec::new(), // No dynamic data to care about
        profiles,
        recipes,
    })
}

/// Build one profile per server
fn build_profiles(
    servers: Vec<Server>,
) -> IndexMap<ProfileId, Profile<Expression>> {
    servers
        .into_iter()
        .map(|Server { url, variables, .. }| {
            let id: ProfileId = url.clone().into();
            // Include a "host" variable for each server, but allow the
            // user-defined variables to override that
            let data =
                iter::once(("host".to_owned(), url.clone().into()))
                    .chain(variables.into_iter().flatten().map(
                        |(name, variable)| (name, variable.default.into()),
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
) -> Result<RecipeTree<Expression>, DuplicateRecipeIdError> {
    let reference_resolver = ReferenceResolver::new(components);
    let mut recipes: IndexMap<RecipeId, RecipeNode<Expression>> =
        IndexMap::new();

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
                RecipeNode::Folder(Folder {
                    id: folder_id,
                    name: Some(tag),
                    children: IndexMap::default(),
                })
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
                error!("References not supported for path items `{reference}`")
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
    /// We're going to build up a template literal incrementally, then we'll
    /// convert to an expression at the end
    url: Vec<TemplateChunk>,
    body: Option<RecipeBody<Expression>>,
    authentication: Option<Authentication<Expression>>,
    query: Vec<(String, Expression)>,
    headers: IndexMap<String, Expression>,
    reference_resolver: &'a ReferenceResolver,
}

impl<'a> RecipeBuilder<'a> {
    /// Translate an OpenAPI Operation into a recipe
    fn build_recipe(
        operation: Operation,
        reference_resolver: &'a ReferenceResolver,
        path_name: &str,
        method: HttpMethod,
    ) -> Recipe<Expression> {
        // Use operation_id if one is provided, otherwise generate one
        let id: RecipeId = operation
            .operation_id
            .unwrap_or_else(|| format!("{path_name}-{method}"))
            .into();
        let name = operation.summary.unwrap_or_else(|| path_name.to_owned());

        // Build a URL from the path name. The `host` profile field should have
        // been defined separately from the `servers` block of the OpenAPI spec.
        // The template will look like: `${profile("host")}/path`
        // This may be modified later on to convert path parameters into
        // expression chunks
        let url: Vec<TemplateChunk> =
            vec![profile_field("host").into_expr().into(), path_name.into()];

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

        Recipe {
            id: builder.id,
            persist: true,
            name: Some(builder.name),
            method: builder.method,
            url: TemplateLiteral::new(builder.url).into(),
            body: builder.body,
            authentication: builder.authentication,
            query: build_query_parameters(builder.query),
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
                    .resolve::<Parameter>(parameter)
                    .with_context(|| format!("{id}.parameters", id = self.id))
                    .traced()
                    .ok()
            })
            .for_each(|parameter| match parameter.into_owned() {
                Parameter::Query { parameter_data, .. } => {
                    self.query.push((parameter_data.name, "".into()));
                }
                Parameter::Header { parameter_data, .. } => {
                    // if the name field is "Accept", "Content-Type" or
                    // "Authorization", the parameter definition SHALL be
                    // ignored. https://spec.openapis.org/oas/v3.0.3#fixed-fields-9
                    let name = parameter_data.name;
                    match name.as_str() {
                        "Accept" | "Content-Type" | "Authorization" => {}
                        _ => {
                            self.headers.insert(name, "".into());
                        }
                    }
                }
                Parameter::Path {
                    style: PathStyle::Simple,
                    parameter_data,
                } => replace_path_param(&mut self.url, &parameter_data.name),
                Parameter::Path {
                    style: PathStyle::Matrix | PathStyle::Label,
                    parameter_data,
                } => {
                    error!(
                        "Unsupported type for path param `{}`",
                        parameter_data.name
                    )
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
            .flat_map(|scheme| scheme.into_keys())
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
                            username: "username".into(),
                            password: "password".into(),
                        });
                    }
                    "Bearer" | "bearer" => {
                        let token = bearer_format
                            .clone()
                            .unwrap_or_default()
                            .into();
                        self.authentication =
                            Some(Authentication::Bearer{token});
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
                        profile_field("api_key").into()
                    )),
                    APIKeyLocation::Header => {
                        self.headers.insert(
                            name.clone(),
                            profile_field("api_key").into()
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
        let Ok(request_body) = self
            .reference_resolver
            .resolve::<RequestBody>(request_body)
            .with_context(|| format!("{}.requestBody", self.id))
            .traced()
        else {
            return;
        };

        let body = request_body
            .content
            .iter()
            // Parse each MIME type
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
                self.get_body(&mime, value)
                    .with_context(|| format!("{}.requestBody.{mime}", self.id))
                    .traced()
                    .ok()
            })
            // Sort known content types first (false < true)
            .sorted_by_key(|body| matches!(body, RecipeBody::Raw { .. }))
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
                        .resolve(example.clone())
                        .with_context(|| {
                            format!(
                                "{id}.requestBody.content.{mime_}\
                                .examples.{name}",
                                id = self.id,
                            )
                        })
                        .traced()
                        .ok()?
                        .into_owned();
                    example.value
                });
        let schema_example = media_type.schema.as_ref().and_then(|schema| {
            let schema = self
                .reference_resolver
                .resolve::<Schema>(schema.clone())
                .with_context(|| {
                    format!(
                        "{id}.requestBody.content.{mime}.schema",
                        id = self.id,
                    )
                })
                .traced()
                .ok()?;
            schema.schema_data.example.clone()
        });

        example.into_iter().chain(examples).chain(schema_example)
    }

    /// Convert a body of a single example to a recipe body
    fn get_body(
        &self,
        mime: &Mime,
        body: serde_json::Value,
    ) -> anyhow::Result<RecipeBody<Expression>> {
        fn unwrap_object(
            value: serde_json::Value,
        ) -> anyhow::Result<IndexMap<String, Expression>> {
            // This may not be correct, but we'll just stringify the value as
            // JSON https://swagger.io/docs/specification/describing-request-body/multipart-requests/
            if let serde_json::Value::Object(object) = value {
                Ok(object
                    .into_iter()
                    .map(|(key, value)| {
                        // Convert value to string
                        let value = match value {
                            serde_json::Value::String(s) => s.into(),
                            // Do *not* prettify here; we want to be able to
                            // show this in one line in the UI
                            _ => value.to_string().into(),
                        };
                        (key, value)
                    })
                    .collect())
            } else {
                Err(anyhow!("Expected object"))
            }
        }

        if mime == &mime::APPLICATION_JSON {
            // Currently we don't match against any JSON extensions. Just a
            // shortcut, could fix later
            Ok(RecipeBody::Json {
                data: Json::new(body).into(),
            })
        } else if mime == &mime::APPLICATION_WWW_FORM_URLENCODED {
            let form = unwrap_object(body)?;
            Ok(RecipeBody::FormUrlencoded { data: form })
        } else if mime == &mime::MULTIPART_FORM_DATA {
            let form = unwrap_object(body)?;
            Ok(RecipeBody::FormMultipart { data: form })
        } else {
            warn!(
                "Unknown content type `{mime}` for body of recipe `{}`",
                self.id
            );
            Ok(RecipeBody::Raw {
                data: format!("{body:#}").into(),
            })
        }
    }
}

/// Replace a path parameter `p` with a call to `profile("p")`. The field
/// probably actually be in the profile, but it's better than being a completely
/// invalid URL. We have no way of knowing how the user actually wants to fill
/// this value.
#[expect(unstable_name_collisions)]
fn replace_path_param(url: &mut Vec<TemplateChunk>, parameter: &str) {
    // This string looks like `{parameter}`
    let pattern = format!("{{{}}}", parameter);

    // Replace any occurrence of `{parameter}` with a dynamic chunk. We're going
    // to be adding chunks as we go, so we can't use a for loop.
    let mut i = 0;
    while i < url.len() {
        // unstable: if-let chain
        if let TemplateChunk::Literal(s) = &url[i] {
            // This is a bit ugly but it works well enough and the performance
            // isn't important
            let new_chunks = s
                // Replace instances of `{id}` with expression chunks
                .split(&pattern)
                .map(|s| TemplateChunk::Literal(s.into()))
                .intersperse(TemplateChunk::expression(
                    profile_field(parameter).into_expr(),
                ))
                // Remove lingering empty chunks
                .filter(|chunk| match chunk {
                    TemplateChunk::Literal(s) => !s.is_empty(),
                    TemplateChunk::Expression(_) => true,
                })
                // Collect is necessary to break the lifetime on s
                .collect_vec();

            let len = new_chunks.len();
            // Replace the chunk at i with 1+ new chunks
            url.splice(i..=i, new_chunks);
            // Jump past all the chunks we just inserted
            i += len;
        } else {
            i += 1;
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
    use slumber_core::ps::ENGINE;
    use slumber_util::test_data_dir;
    use std::{path::PathBuf, sync::OnceLock};

    const OPENAPIV3_FILE: &str = "openapiv3_petstore.yml";
    /// Assertion expectation is stored in a separate file. This is for a couple
    /// reasons:
    /// - It's huge so it makes code hard to navigate
    /// - Changes don't require a re-compile
    const OPENAPIV3_EXPECTED_FILE: &str = "openapiv3_petstore_expected.js";

    /// Catch-all test for openapiv3 import
    #[rstest]
    fn test_openapiv3_import(test_data_dir: PathBuf) {
        // Convert the external collection into a PS AST, then parse the
        // expected file into an AST and compare the two
        let imported = from_openapi(test_data_dir.join(OPENAPIV3_FILE))
            .unwrap()
            .into_petitscript();
        let expected = ENGINE
            .parse(test_data_dir.join(OPENAPIV3_EXPECTED_FILE))
            .unwrap();
        assert_eq!(&imported, expected.data());
    }

    #[rstest]
    #[case::none("/path", &[], &["/path".into()])]
    // Typically there would be a / in the path but test this to be safe
    #[case::id_only("{id}", &["id"], &[path_chunk("id")])]
    #[case::id_first("{id}/path", &["id"], &[path_chunk("id"), "/path".into()])]
    #[case::id_last("/path/{id}", &["id"], &["/path/".into(), path_chunk("id")])]
    #[case::dupe_id(
        "/{id}/{id}",
        &["id"],
        &["/".into(), path_chunk("id"), "/".into(), path_chunk("id")],
    )]
    #[case::two_params(
        "/{id1}/{id2}",
        &["id1", "id2"],
        &["/".into(), path_chunk("id1"), "/".into(), path_chunk("id2")],
    )]
    fn test_replace_path_param(
        #[case] path: &str,
        #[case] parameters: &[&str],
        #[case] expected_chunks: &[TemplateChunk],
    ) {
        let mut chunks = vec![path.into()];
        for parameter in parameters {
            replace_path_param(&mut chunks, parameter);
        }
        assert_eq!(&chunks, expected_chunks);
    }

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
        json_body(json!({"field": "value"})),
    )]
    #[case::form_urlencoded(
        [(
            "application/x-www-form-urlencoded",
            MediaType {
                example: Some(json!({"field": "value", "complex": [1, 2]})),
                ..Default::default()
            }
        )],
        RecipeBody::FormUrlencoded {
            data: indexmap! {
                // Complex value gets stringified
                "complex".into() => "[1,2]".into(),
                "field".into() => "value".into(),
            },
        },
    )]
    #[case::form_multipart(
        [(
            "multipart/form-data",
            MediaType {
                example: Some(json!({"field": "value", "complex": [1, 2]})),
                ..Default::default()
            }
        )],
        RecipeBody::FormMultipart {
            data: indexmap! {
                // Complex value gets stringified
                "complex".into() => "[1,2]".into(),
                "field".into() => "value".into(),
            },
        },
    )]
    #[case::raw(
        [(
            "application/xml",
            MediaType {
                example: Some(json!({"field": "value"})),
                ..Default::default()
            }
        )],
        RecipeBody::Raw { data: "{\n  \"field\": \"value\"\n}".into() },
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
        json_body(json!({"field": "value"})),
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
        json_body(json!({"field": "value"})),
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
        json_body(json!({"field": "example"})),
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
        json_body(json!({"field": "value"})),
    )]
    fn test_process_body(
        mut builder: RecipeBuilder<'static>,
        #[case] body_content: impl IntoIterator<Item = (&'static str, MediaType)>,
        #[case] expected: RecipeBody<Expression>,
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
            url: vec![profile_field("host").into_expr().into(), "/get".into()],
            body: None,
            authentication: None,
            query: Default::default(),
            headers: Default::default(),
            reference_resolver: RESOLVER
                .get_or_init(|| ReferenceResolver::new(None)),
        }
    }

    fn json_body(value: serde_json::Value) -> RecipeBody<Expression> {
        RecipeBody::Json {
            data: Json::new(value).into(),
        }
    }

    fn path_chunk(parameter: &str) -> TemplateChunk {
        TemplateChunk::expression(profile_field(parameter).into())
    }
}
