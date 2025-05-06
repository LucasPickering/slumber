//! Import request collections from VSCode `.rest` files or Jetbrains `.http`
//! files. VSCode: https://github.com/Huachao/vscode-restclient
//! Jetbrains: https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html

use crate::{
    ImportCollection,
    common::{Json, build_template},
};
use indexmap::IndexMap;
use mime::Mime;
use petitscript::ast::{Expression, TemplateChunk};
use reqwest::header;
use rest_parser::{
    Body as RestBody, RestFlavor, RestFormat, RestRequest, RestVariables,
    headers::Authorization as RestAuthorization,
    template::{Template, TemplatePart},
};
use slumber_core::{
    collection::{
        Authentication, Profile, ProfileId, QueryParameterValue, Recipe,
        RecipeBody, RecipeId, RecipeNode, RecipeTree,
    },
    http::HttpMethod,
    petit,
};
use std::path::Path;
use tracing::error;

/// Convert a VSCode `.rest` file or a Jetbrains `.http` file into a slumber
/// collection
pub fn from_rest(
    rest_file: impl AsRef<Path>,
) -> anyhow::Result<ImportCollection> {
    let rest_file = rest_file.as_ref();
    // Parse the file and determine the flavor using the extension
    let rest_format = RestFormat::parse_file(rest_file)?;

    let RestFormat {
        requests,
        variables,
        flavor,
    } = rest_format;

    let recipes = build_recipe_tree(requests, &variables);
    let profiles = build_profiles(flavor, variables);

    Ok(ImportCollection {
        declarations: Vec::new(),
        profiles,
        recipes,
    })
}

/// There is no profile system in Rest, here is a default to use
fn build_profiles(
    flavor: RestFlavor,
    variables: RestVariables,
) -> IndexMap<ProfileId, Profile<Expression>> {
    let (flavor_name, flavor_id) = flavor_name_and_id(flavor);
    let profile_id: ProfileId = flavor_id.into();
    let default_profile = Profile {
        id: profile_id.clone(),
        name: Some(flavor_name),
        default: true,
        data: map_values(variables),
    };

    IndexMap::from([(profile_id.clone(), default_profile)])
}

/// Build each individual recipe and combine them into a flat tree
fn build_recipe_tree(
    requests: Vec<RestRequest>,
    variables: &RestVariables,
) -> RecipeTree<Expression> {
    let recipes = requests
        .into_iter()
        .enumerate()
        .map(|(index, request)| build_recipe(request, index, variables))
        .map(|recipe| (recipe.id.clone(), RecipeNode::Recipe(recipe)))
        .collect::<IndexMap<_, _>>();
    // Error is impossible here because there are no folders and the map above
    // already enforces uniqueness
    RecipeTree::new(recipes).unwrap()
}

fn build_recipe(
    request: RestRequest,
    index: usize,
    variables: &RestVariables,
) -> Recipe<Expression> {
    // Add the index to prevent duplicate ID error
    let id: RecipeId = format!(
        "{name}_{index}",
        name = request.name.as_deref().unwrap_or("request")
    )
    .into();

    // Slumber doesn't support template methods, so we fill in now
    let rendered_method = request.method.render(variables);

    // The rest parser does not enforce method names. If a method is invalid,
    // log it and fall back to GET
    let method: HttpMethod = rendered_method.parse().unwrap_or_else(|_| {
        error!(
            "Invalid HTTP method for request {id}. GET will be used instead"
        );
        HttpMethod::Get
    });
    let url = template_to_expression(request.url);
    // Read MIME type from the Content-Type header. If the value is dynamic we
    // can't read it now
    let mime: Option<Mime> = request
        .headers
        .get(header::CONTENT_TYPE.as_str())
        .and_then(|header_value| header_value.raw.parse().ok());
    let headers = map_values(request.headers);
    let authentication = request.authorization.map(build_authentication);
    let query = build_query(request.query);
    let body = request.body.map(|body| build_body(body, mime));

    Recipe {
        id,
        persist: true,
        name: request.name,
        method,
        url,
        authentication,
        body,
        headers,
        query,
    }
}

/// Convert REST Authentication to Slumber Authentication
fn build_authentication(
    authentication: RestAuthorization,
) -> Authentication<Expression> {
    match authentication {
        RestAuthorization::Bearer(bearer) => Authentication::Bearer {
            token: bearer.into(),
        },
        RestAuthorization::Basic { username, password } => {
            Authentication::Basic {
                username: username.into(),
                password: password.unwrap_or_default().into(),
            }
        }
    }
}

/// Build the query variables
fn build_query(
    query: IndexMap<String, Template>,
) -> IndexMap<String, QueryParameterValue<Expression>> {
    query
        .into_iter()
        .map(|(k, v)| {
            (k, QueryParameterValue::Single(template_to_expression(v)))
        })
        .collect()
}

/// Build a recipe body. The mime type from the `Content-Type` header informs
/// the type of body returned
fn build_body(body: RestBody, mime: Option<Mime>) -> RecipeBody<Expression> {
    // We only want the text for now
    let text = match body {
        RestBody::Text(text)
        // We don't have a way to define saving to a file within a recipe, so
        // just treat it as a normal text body
        | RestBody::SaveToFile { text, .. } => text,
        RestBody::LoadFromFile { filepath, .. } => {
            // Load body from a file by calling the file() function. We can't
            // access the body yet so we can't parse it into a special body
            // type; just treat it as raw text
            let path = template_to_expression(filepath);
            return RecipeBody::Raw {
                data: petit::call_fn("file", [path], []).into(),
            };
        }
    };

    // TODO support multipart forms. Need to figure out how the REST format
    // defines them
    if mime == Some(mime::APPLICATION_JSON) {
        // Even though the text has already been parsed as a template, we'll
        // need to treat it as raw text to parse it to JSON. Then we'll parse
        // each inner string as a template again.
        let value = match serde_json::from_str(&text.raw) {
            Ok(value) => value,
            Err(err) => {
                // Treat it as raw text
                error!("Invalid JSON body: {err}");
                return RecipeBody::Raw {
                    data: template_to_expression(text),
                };
            }
        };
        let expression: Expression = Json {
            value,
            convert_string: |s| match s.parse::<Template>() {
                Ok(template) => template_to_expression(template),
                // The string has already parsed as a template once during the
                // initial collection load, so a failure is very unlikely here.
                // It's theoretically possible though if there's a weird
                // interaction with JSON object syntax. In that case, just use
                // the literal string
                Err(_) => s.into(),
            },
        }
        .into();
        RecipeBody::Json { data: expression }
    } else if mime == Some(mime::APPLICATION_WWW_FORM_URLENCODED) {
        // Parse the body as a URL-encoded form
        let form: IndexMap<String, String> =
            match serde_urlencoded::from_str(&text.raw) {
                Ok(value) => value,
                Err(err) => {
                    // Treat it as raw text
                    error!("Invalid url-encoded body: {err}");
                    return RecipeBody::Raw {
                        data: template_to_expression(text),
                    };
                }
            };
        let data = form
            .into_iter()
            .map(|(field, value)| {
                // It's unlikely any individual field will fail to parse because
                // the REST parser successfully parsed the whole thing as a
                // template. But if it does, just use the literal value
                let expression = value
                    .parse::<Template>()
                    .map(template_to_expression)
                    .unwrap_or_else(|_| value.into());
                (field, expression)
            })
            .collect();
        RecipeBody::FormUrlencoded { data }
    } else {
        // Unknown MIME type - treat it as raw text
        RecipeBody::Raw {
            data: template_to_expression(text),
        }
    }
}

/// Convert a map of REST templates to Slumber expressions
fn map_values(
    template_map: IndexMap<String, Template>,
) -> IndexMap<String, Expression> {
    template_map
        .into_iter()
        .map(|(k, v)| (k, template_to_expression(v)))
        .collect()
}

/// Convert a REST Template into a PetitScript template expression
fn template_to_expression(template: Template) -> Expression {
    let chunks = template.parts.into_iter().map(|part| match part {
        TemplatePart::Text(text) => TemplateChunk::Literal(text),
        TemplatePart::Variable(field) => petit::profile_chunk(field),
    });
    build_template(chunks)
}

fn flavor_name_and_id(flavor: RestFlavor) -> (String, String) {
    let (name, id) = match flavor {
        RestFlavor::Jetbrains => ("Jetbrains HTTP File", "http_file"),
        RestFlavor::Vscode => ("VSCode Rest File", "rest_file"),
        RestFlavor::Generic => ("Rest File", "rest_file"),
    };
    (name.into(), id.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use petitscript::{Engine, ast::ObjectLiteral};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const REST_FILE: &str = "rest_http_bin.http";
    const REST_EXPECTED_FILE: &str = "rest_expected.js";

    fn example_vars() -> RestVariables {
        IndexMap::from([
            ("HOST".into(), Template::new("https://httpbin.org")),
            ("FIRST_NAME".into(), Template::new("John")),
            ("LAST_NAME".into(), Template::new("Smith")),
            (
                "FULL_NAME".into(),
                Template::new("{{FIRST_NAME}} {{LAST_NAME}}"),
            ),
        ])
    }

    #[rstest]
    fn test_rest_import(test_data_dir: PathBuf) {
        // Convert the external collection into a PS AST, then parse the
        // expected file into an AST and compare the two
        let imported = from_rest(test_data_dir.join(REST_FILE))
            .unwrap()
            .into_petitscript();
        let expected = Engine::default()
            .parse(test_data_dir.join(REST_EXPECTED_FILE))
            .unwrap();
        assert_eq!(&imported, expected.data());
    }

    /// If a request has an unknown method, it gets replaced with gET
    #[test]
    fn test_invalid_http_method() {
        let test_req = RestRequest {
            url: Template::new("{{HOST}}/get"),
            method: Template::new("INVALID"),
            ..RestRequest::default()
        };

        let recipe = build_recipe(test_req, 0, &IndexMap::new());
        assert_eq!(recipe.method, HttpMethod::Get);
    }

    #[test]
    fn test_file_body() {
        let test_req = RestRequest {
            url: Template::new("{{HOST}}/post"),
            method: Template::new("POST"),
            headers: IndexMap::from([(
                "content-type".into(),
                Template::new("application/json"),
            )]),
            body: Some(RestBody::LoadFromFile {
                process_variables: false,
                encoding: None,
                filepath: Template::new("./test_data/rest_pets.json"),
            }),
            ..RestRequest::default()
        };

        let recipe = build_recipe(test_req, 0, &IndexMap::new());
        assert_eq!(
            recipe.body,
            Some(RecipeBody::Raw {
                data: petit::call_fn(
                    "file",
                    ["./test_data/rest_pets.json".into()],
                    []
                )
                .into()
            })
        );
    }

    #[test]
    fn test_raw_body() {
        let test_req = RestRequest {
            url: Template::new("{{HOST}}/post"),
            method: Template::new("POST"),
            body: Some(RestBody::Text(Template::new("test data"))),
            ..RestRequest::default()
        };

        let recipe = build_recipe(test_req, 0, &example_vars());

        let body = recipe.body.unwrap();
        assert_eq!(
            body,
            RecipeBody::Raw {
                data: "test data".into(),
            }
        );
    }

    #[test]
    fn test_json_body() {
        let test_req = RestRequest {
            url: Template::new("{{HOST}}/post"),
            method: Template::new("POST"),
            headers: IndexMap::from([(
                "content-type".into(),
                Template::new("application/json"),
            )]),
            body: Some(RestBody::Text(Template::new(
                r#"{"animal": "penguin", "name": "{{FIRST}}"}"#,
            ))),
            ..RestRequest::default()
        };

        let recipe = build_recipe(test_req, 0, &example_vars());

        let body = recipe.body.unwrap();
        assert_eq!(
            body,
            RecipeBody::Json {
                data: ObjectLiteral::new([
                    ("animal", "penguin".into()),
                    // Nested template should be parsed
                    ("name", petit::profile_field("FIRST").into()),
                ])
                .into()
            }
        );
    }

    #[test]
    fn test_form_urlencoded_body() {
        let test_req = RestRequest {
            url: Template::new("{{HOST}}/post"),
            method: Template::new("POST"),
            headers: IndexMap::from([(
                "content-type".into(),
                Template::new("application/x-www-form-urlencoded"),
            )]),
            body: Some(RestBody::Text(Template::new(
                "first={{FIRST}}&last={{LAST}}",
            ))),
            ..RestRequest::default()
        };

        let recipe = build_recipe(test_req, 0, &example_vars());

        let body = recipe.body.unwrap();
        assert_eq!(
            body,
            RecipeBody::FormUrlencoded {
                data: IndexMap::from([
                    ("first".into(), petit::profile_field("FIRST").into()),
                    ("last".into(), petit::profile_field("LAST").into())
                ])
            }
        );
    }
}
