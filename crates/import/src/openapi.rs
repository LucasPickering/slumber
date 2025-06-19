//! Import request collections from an OpenAPI v3.0 or v3.1 specification.
//!
//! - Servers are mapped to profiles
//!     - URL of the server is stored in the `host` field
//! - Operations (i.e. path-method pairs) are mapped to recipes
//! - Tags are mapped to folders
//!     - Since tags are m2m but folders are o2m, we only take the first tag
//! - References are resolved within the same file. We don't support resolving
//!   from other files.
//!
//! OpenAPI is not semver compliant (a change they helpfully made in in a minor
//! version), and 3.1 is not backward compatible with 3.0. We have two separate
//! importers because each we use one library that only supports 3.0 and one
//! that only supports 3.1.

mod resolve;
mod v3_0;

use crate::ImportInput;
use anyhow::{Context, anyhow};
use slumber_core::collection::Collection;
use slumber_util::NEW_ISSUE_LINK;
use tracing::warn;

/// Loads a collection from an OpenAPI v3 specification file
pub async fn from_openapi(input: &ImportInput) -> anyhow::Result<Collection> {
    warn!(
        "The OpenAPI importer is approximate. Some features are missing \
            and it may not give you an equivalent or fulling functional
            collection. If you encounter a bug or would like to request support
            for a particular OpenAPI feature, please open an issue:
            {NEW_ISSUE_LINK}"
    );

    // Read the spec into YAML and use the `version` field to determine which
    // importer to use. The format can be YAML or JSON, so we can just treat it
    // all as YAML
    let content = input.load().await?;
    let yaml = serde_yaml::from_str(&content)
        .context("Error deserializing OpenAPI collection")?;

    let version =
        get_version(&yaml).ok_or_else(|| anyhow!("Missing OpenAPI version"))?;
    if version.starts_with("3.0.") {
        v3_0::from_openapi_v3_0(yaml)
    } else {
        Err(anyhow!(
            "Unsupported OpenAPI version. Only OpenAPI 3.0 is supported"
        ))
    }
}

<<<<<<< HEAD
fn get_version(yaml: &serde_yaml::Value) -> Option<&str> {
    if let serde_yaml::Value::Mapping(mapping) = yaml {
        mapping.get("openapi").and_then(|v| v.as_str())
    } else {
        None
=======
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
        let url = format!("{{{{host}}}}{path_name}");

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
                            username: Template::from_field("username".into()),
                            password: (Template::from_field(
                                "password".into(),
                            )),
                        });
                    }
                    "Bearer" | "bearer" => {
                        let template = bearer_format
                            .clone()
                            .map(Template::raw)
                            .unwrap_or_default();
                        self.authentication =
                            Some(Authentication::Bearer(template));
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
                        Template::from_field("api_key".into()),
                    )),
                    APIKeyLocation::Header => {
                        self.headers.insert(
                            name.clone(),
                            Template::from_field("api_key".into()),
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
                matches!(
                    body,
                    RecipeBody::Raw {
                        content_type: None,
                        ..
                    }
                )
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
            Ok(RecipeBody::Raw {
                body: Template::raw(format!("{body:#}")),
                content_type: None,
            })
        }
>>>>>>> 8d01b467 (Refactor request overrides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_core::collection::Collection;
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const OPENAPI_V3_0_FILE: &str = "openapi_v3_0_petstore.yml";
    const OPENAPI_V3_0_IMPORTED_FILE: &str =
        "openapi_v3_0_petstore_imported.yml";

    /// Catch-all test for OpenAPI v3.0 import
    #[rstest]
    #[tokio::test]
    async fn test_openapiv3_0_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(OPENAPI_V3_0_FILE));
        let imported = from_openapi(&input).await.unwrap();
        let expected =
            Collection::load(&test_data_dir.join(OPENAPI_V3_0_IMPORTED_FILE))
                .unwrap();
        assert_eq!(imported, expected);
    }
}
