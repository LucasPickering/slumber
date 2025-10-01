use crate::common;
use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use itertools::Itertools;
use mime::Mime;
use oas3::spec::{
    FromRef, MediaType, MediaTypeExamples, ObjectOrReference, ObjectSchema,
    Operation, Parameter, ParameterIn, PathItem, RequestBody, Schema,
    SchemaType, SchemaTypeSet, SecurityRequirement, SecurityScheme, Server,
    Spec,
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
use std::{iter, mem};
use strum::IntoEnumIterator;
use tracing::{debug, error, warn};

/// Loads a collection from an OpenAPI v3.1 specification
pub fn from_openapi_v3_1(
    spec: serde_yaml::Value,
) -> anyhow::Result<Collection> {
    let mut spec: Spec = serde_yaml::from_value(spec)
        .context("Error deserializing OpenAPI 3.1 collection")?;

    let name = mem::take(&mut spec.info.title);
    let profiles = build_profiles(
        // We don't need the servers anywhere else so we can move them out to
        // avoid a clone
        mem::take(&mut spec.servers),
    );
    let recipes = build_recipe_tree(spec)?;

    Ok(Collection {
        name: Some(name),
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
                    .chain(variables.into_iter().map(|(name, variable)| {
                        (name, Template::raw(variable.default))
                    }))
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
fn build_recipe_tree(spec: Spec) -> Result<RecipeTree, DuplicateRecipeIdError> {
    let reference_resolver = ReferenceResolver::new(&spec);
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
    for (path, item) in spec.paths.iter().flatten() {
        // Note: we may fuck up the ordering of operations here. The
        // ordering is already lost because the openapi lib deserializes
        // into static fields instead of a map, so there's nothing we
        // can do to preserve the old order
        for method in HttpMethod::iter() {
            if let Some(op) = get_operation(item, method) {
                add_recipe(path, op, method);
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
fn get_operation(
    path_item: &PathItem,
    method: HttpMethod,
) -> Option<Operation> {
    match method {
        HttpMethod::Connect => None,
        HttpMethod::Delete => path_item.delete.clone(),
        HttpMethod::Get => path_item.get.clone(),
        HttpMethod::Head => path_item.head.clone(),
        HttpMethod::Options => path_item.options.clone(),
        HttpMethod::Patch => path_item.patch.clone(),
        HttpMethod::Post => path_item.post.clone(),
        HttpMethod::Put => path_item.put.clone(),
        HttpMethod::Trace => path_item.trace.clone(),
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
    reference_resolver: &'a ReferenceResolver<'a>,
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
    fn process_parameters(
        &mut self,
        parameters: Vec<ObjectOrReference<Parameter>>,
    ) {
        parameters
            .into_iter()
            .filter_map(|parameter| {
                // For any reference that fails to resolve, print the error and
                // throw it away
                self.reference_resolver.resolve::<Parameter>(&parameter)
            })
            .for_each(|parameter| match parameter.location {
                ParameterIn::Query => {
                    self.query.push((parameter.name, Template::default()));
                }
                ParameterIn::Header => {
                    // if the name field is "Accept", "Content-Type" or
                    // "Authorization", the parameter definition SHALL be
                    // ignored. https://spec.openapis.org/oas/v3.0.3#fixed-fields-9
                    let name = parameter.name;
                    match name.as_str() {
                        "Accept" | "Content-Type" | "Authorization" => {}
                        _ => {
                            self.headers.insert(name, Template::default());
                        }
                    }
                }
                ParameterIn::Path => {
                    // Replace path params with a template key. The key probably
                    // won't refer to anything, but it's better than being
                    // completely invalid. We have no way of knowing how the
                    // user actually wants to fill this
                    // value
                    let id = parameter.name;
                    // {id} -> {{id}}
                    self.url = self.url.replace(
                        &format!("{{{id}}}"),
                        &format!("{{{{{id}}}}}"),
                    );
                }
                ParameterIn::Cookie => {
                    error!(
                        "Unsupported parameter type cookie for param `{}`",
                        parameter.name
                    );
                }
            });
    }

    /// Imperatively update the recipe to include security scheme(s). Depending
    /// on the scheme this may map to first-class auth, query params, or headers
    fn process_security(&mut self, security: Vec<SecurityRequirement>) {
        security
            .into_iter()
            .flat_map(|s| s.0)
            // Resolve references, throwing away invalid ones
            .filter_map(|(scheme_name, _)| {
                self.reference_resolver.security_schema(&scheme_name)
            })
            .for_each(|scheme| self.process_security_scheme(scheme));
    }

    /// Imperatively update the recipe according to a single security scheme
    /// <https://spec.openapis.org/oas/v3.1.0.html#security-scheme-object>
    fn process_security_scheme(&mut self, security_scheme: SecurityScheme) {
        // Where we need an auth value but don't have one, we'll create
        // a placeholder template. This should improve discoverability for
        // template features. We don't expect the fields to actually map to
        // anything

        match security_scheme {
            SecurityScheme::ApiKey { location, name, .. } => {
                match location.as_str() {
                    "header" => {
                        self.headers.insert(
                            name.clone(),
                            Template::from_field("api_key"),
                        );
                    }
                    "query" => {
                        self.query.push((
                            name.clone(),
                            Template::from_field("api_key"),
                        ));
                    }
                    _ => error!("Unsupported API key location `{location}`"),
                }
            }
            SecurityScheme::Http { scheme, .. } => match scheme.as_str() {
                "basic" => {
                    self.authentication = Some(Authentication::Basic {
                        username: Template::from_field("username"),
                        password: Some(Template::from_field("password")),
                    });
                }
                "bearer" => {
                    self.authentication = Some(Authentication::Bearer {
                        token: Template::from_field("api_token"),
                    });
                }
                _ => {
                    error!("Unsupported HTTP authentication scheme `{scheme}`");
                }
            },
            SecurityScheme::OAuth2 { .. } => {
                error!("Unsupported security scheme: OAuth2");
            }
            SecurityScheme::OpenIdConnect { .. } => {
                error!("Unsupported security scheme: OpenIDConnect");
            }
            SecurityScheme::MutualTls { .. } => {
                error!("Unsupported security scheme: Mutual TLS");
            }
        }
    }

    /// Imperatively set the request body. The body can contain multiple
    /// examples, either on the operation itself or the underlying schema. We'll
    /// grab the first valid one.
    fn process_body(&mut self, request_body: ObjectOrReference<RequestBody>) {
        // If the reference is invalid, it'll be logged and we can fuck off
        let Some(request_body) = self
            .reference_resolver
            .resolve::<RequestBody>(&request_body)
        else {
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
                self.get_examples(media_type)
                    .into_iter()
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
    /// - `media_type.examples` (mutually exclusive with `media_type.example`)
    /// - `media_type.schema.examples`
    /// - `media_type.schema.properties` (build an example from the schema def)
    fn get_examples(
        &'a self,

        media_type: &'a MediaType,
    ) -> Vec<serde_json::Value> {
        // These are ordered by precedence. If any is empty we'll fall back to
        // the next one
        match &media_type.examples {
            Some(MediaTypeExamples::Example { example }) => {
                vec![example.clone()]
            }
            Some(MediaTypeExamples::Examples { examples }) => examples
                .values()
                .filter_map(|example| {
                    let example = self.reference_resolver.resolve(example)?;
                    example.value
                })
                .collect(),
            // If there was no example for the operation, look at the underlying
            // schema
            None => {
                let example = media_type
                    .schema
                    .as_ref()
                    // Resolve the reference
                    .and_then(|schema| self.reference_resolver.resolve(schema))
                    .map(|schema| {
                        schema
                            .example
                            .clone()
                            // If there's no example declared on the schema,
                            // generate one from its properties
                            .unwrap_or_else(|| self.schema_to_json(&schema))
                    });

                // This is an option, so we get a vec of 0 or 1
                example.into_iter().collect()
            }
        }
    }

    /// Build an example JSON value from a schema definition. This will be
    /// called recursively to convert individual parts of complex bodies
    fn schema_to_json(&self, schema: &ObjectSchema) -> serde_json::Value {
        // If an example value exists, use it
        if let Some(example) = &schema.example {
            return example.clone();
        }

        // Otherwise if an enum is given, pull the first value
        if let Some(value) = schema.enum_values.first() {
            return value.clone();
        }

        // If anyOf, allOf, or oneOf is given, grab the first value from that
        if let Some(inner_schema) = schema
            .any_of
            .iter()
            .chain(schema.all_of.iter())
            .chain(schema.one_of.iter())
            .next()
            .and_then(|schema| self.reference_resolver.resolve(schema))
        {
            return self.schema_to_json(&inner_schema);
        }

        // Derive a value from the schema type
        let schema_type = schema
            .schema_type
            .as_ref()
            .and_then(|type_set| match type_set {
                SchemaTypeSet::Single(schema_type) => Some(*schema_type),
                SchemaTypeSet::Multiple(items) => items.first().copied(),
            })
            .unwrap_or(SchemaType::Object);
        match schema_type {
            SchemaType::Null => serde_json::Value::Null,
            SchemaType::Boolean => false.into(),
            SchemaType::Integer | SchemaType::Number => {
                // Try minimum or maximum to ensure the value is valid
                schema
                    .minimum
                    .as_ref()
                    .or(schema.maximum.as_ref())
                    .cloned()
                    // Fallback to a default
                    .unwrap_or(0.into())
                    .into()
            }
            // Just use default for a string. We could try to come up with
            // something based on the `format` or `pattern` fields but I'm
            // taking a shortcut
            SchemaType::String => "".into(),
            SchemaType::Array => schema
                .items
                .as_ref()
                .and_then(|item_schema| {
                    // Figure out the type of the contained item, and include
                    // one element of it as the prefilled value
                    let item_value: serde_json::Value = match &**item_schema {
                        Schema::Boolean(boolean_schema) => {
                            boolean_schema.0.into()
                        }
                        Schema::Object(object) => {
                            let item_schema =
                                self.reference_resolver.resolve(object)?;
                            self.schema_to_json(&item_schema)
                        }
                    };
                    Some(serde_json::Value::Array(vec![item_value]))
                })
                // No items given, use an empty array
                .unwrap_or_else(|| serde_json::Value::Array(vec![])),
            SchemaType::Object => schema
                .properties
                .iter()
                .filter_map(|(property, schema)| {
                    let schema = self.reference_resolver.resolve(schema)?;
                    let value = self.schema_to_json(&schema);
                    Some((property.clone(), value))
                })
                .collect(),
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

struct ReferenceResolver<'a> {
    spec: &'a Spec,
}

impl<'a> ReferenceResolver<'a> {
    fn new(spec: &'a Spec) -> Self {
        Self { spec }
    }

    /// Resolve an object or reference to an object. If this fails, the error
    /// will be logged and we'll return None. This importer is meant to be
    /// resilient, so errors should be skipped over instead of being fatal, so
    /// there's no need to propagate a Result.
    fn resolve<T>(&self, reference: &ObjectOrReference<T>) -> Option<T>
    where
        T: FromRef,
    {
        reference
            .resolve(self.spec)
            .with_context(|| {
                let ObjectOrReference::Ref { ref_path } = reference else {
                    unreachable!("Only references can fail to resolve")
                };
                format!("Error resolving reference `{ref_path}`")
            })
            .traced()
            .ok()
    }

    /// Look up a security scheme by name. Used to resolve security requirements
    /// in an operation
    fn security_schema(&self, scheme_name: &str) -> Option<SecurityScheme> {
        let reference = self
            .spec
            .components
            .as_ref()?
            .security_schemes
            .get(scheme_name)?;
        self.resolve(reference)
    }
}
