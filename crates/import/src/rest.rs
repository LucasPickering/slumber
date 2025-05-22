//! Import request collections from VSCode `.rest` files or Jetbrains `.http`
//! files. VSCode: https://github.com/Huachao/vscode-restclient
//! Jetbrains: https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html

use anyhow::{Context, anyhow};
use indexmap::IndexMap;
use itertools::Itertools;
use slumber_core::{
    collection::{
        Authentication, Chain, ChainId, ChainOutputTrim, ChainSource,
        Collection, HasId, JsonTemplate, Profile, ProfileId,
        QueryParameterValue, Recipe, RecipeBody, RecipeId, RecipeNode,
        RecipeTree, SelectorMode,
    },
    http::{HttpMethod, content_type::ContentType},
    template::{Identifier, Template},
};

use crate::common;
use reqwest::header;
use rest_parser::{
    Body as RestBody, RestFlavor, RestFormat, RestRequest, RestVariables,
    headers::Authorization as RestAuthorization,
    template::{Template as RestTemplate, TemplatePart as RestTemplatePart},
};
use slumber_util::ResultTraced;
use tracing::error;

use crate::ImportInput;

/// Convert a VSCode `.rest` file or a Jetbrains `.http` file into a slumber
/// collection
pub async fn from_rest(input: &ImportInput) -> anyhow::Result<Collection> {
    let content = input.load().await?;
    let flavor = input
        .file_name()
        .map(RestFlavor::from_path)
        .unwrap_or_default();
    let rest_format = RestFormat::parse(&content, flavor)?;
    Ok(build_collection(rest_format))
}

/// In Rest "Chains" and "Requests" are connected
/// The only chain is loading from a file
#[derive(Debug)]
struct CompleteRecipe {
    recipe: Recipe,
    chain: Option<Chain>,
}

#[derive(Debug)]
struct CompleteBody {
    recipe_body: RecipeBody,
    chain: Option<Chain>,
}

/// Convert a REST Template into a Slumber Template
fn try_build_slumber_template(
    template: RestTemplate,
) -> anyhow::Result<Template> {
    rest_template_to_slumber_string(template)
        .parse()
        .map_err(|err| anyhow!("Failed to parse REST template! {err}"))
}

/// Convert a RESt template to a string that can be parsed into a Slumber
/// template
fn rest_template_to_slumber_string(template: RestTemplate) -> String {
    // Rest templates allow spaces in variables
    // For example `{{ HOST}}` or `{{ HOST }}`
    // These must be removed before putting it through the slumber
    // template parser
    template
        .parts
        .into_iter()
        .map(|part| match part {
            RestTemplatePart::Text(text) => text,
            RestTemplatePart::Variable(var) => {
                "{{".to_string() + var.as_str() + "}}"
            }
        })
        .join("")
}

/// Convert a map of REST templates to Slumber templates (like headers or
/// queries)
/// Errors will be logged and skipped if an invalid template is passed in
fn build_slumber_templates(
    template_map: IndexMap<String, RestTemplate>,
) -> IndexMap<String, Template> {
    template_map
        .into_iter()
        .filter_map(|(k, v)| {
            try_build_slumber_template(v).map(|t| (k, t)).traced().ok()
        })
        .collect()
}

/// Convert REST Authentication to Slumber Authentication
fn build_authentication(r_auth: RestAuthorization) -> Authentication {
    match r_auth {
        RestAuthorization::Bearer(bearer) => {
            Authentication::Bearer(Template::raw(bearer))
        }
        RestAuthorization::Basic { username, password } => {
            Authentication::Basic {
                username: Template::raw(username),
                password: password.map(Template::raw),
            }
        }
    }
}

/// REST supports loading bodies from external files,
/// this is connected to the request.
/// In slumber, this needs to be converted into a request and
/// a chain
fn try_build_chain_from_load_body(
    filepath: RestTemplate,
    recipe_id: &str,
    headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> anyhow::Result<Chain> {
    let full_id = format!("{recipe_id}_body");
    let id: Identifier = Identifier::escape(&full_id);

    let path = try_build_slumber_template(filepath)?;
    let content_type = guess_content_type(headers, variables);

    Ok(Chain {
        id: id.into(),
        content_type,
        trim: ChainOutputTrim::None,
        source: ChainSource::File { path },
        sensitive: false,
        selector: None,
        selector_mode: SelectorMode::Single,
    })
}

/// If the request has JSON headers, mark it as such
fn guess_content_type(
    headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> Option<ContentType> {
    headers
        .iter()
        .any(|(name, value)| {
            name.to_lowercase() == header::CONTENT_TYPE.as_str()
                && value.render(variables) == mime::APPLICATION_JSON.to_string()
        })
        .then_some(ContentType::Json)
}
fn try_build_body(
    body: RestBody,
    recipe_id: &str,
    headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> anyhow::Result<CompleteBody> {
    let (recipe_body, chain) = match body {
        RestBody::Text(text) => {
            let recipe_body = match guess_content_type(headers, variables) {
                Some(ContentType::Json) => {
                    // Parse the body as JSON. Convert the REST template here
                    // to a string that's compatible with Slumber templates
                    let json: JsonTemplate =
                        rest_template_to_slumber_string(text)
                            .parse()
                            .context("Error parsing body as JSON")?;
                    RecipeBody::Json(json)
                }
                None => RecipeBody::Raw(try_build_slumber_template(text)?),
            };
            (recipe_body, None)
        }
        RestBody::SaveToFile { text, .. } => {
            let template = try_build_slumber_template(text)?;
            (RecipeBody::Raw(template), None)
        }
        RestBody::LoadFromFile { filepath, .. } => {
            let chain = try_build_chain_from_load_body(
                filepath, recipe_id, headers, variables,
            )?;
            let template = Template::from_chain(chain.id().clone());
            (RecipeBody::Raw(template), Some(chain))
        }
    };

    Ok(CompleteBody { recipe_body, chain })
}

/// Build the query variables
/// Logging and skipping any invalid templates
fn build_query(
    r_query: IndexMap<String, RestTemplate>,
) -> IndexMap<String, QueryParameterValue> {
    common::build_query_parameters(r_query.into_iter().filter_map(|(k, v)| {
        try_build_slumber_template(v).map(|t| (k, t)).traced().ok()
    }))
}

fn try_build_recipe(
    request: RestRequest,
    index: usize,
    variables: &RestVariables,
) -> anyhow::Result<CompleteRecipe> {
    let name = request.name.unwrap_or("Request".to_string());

    let slug = Identifier::escape(&name);
    // Add the index to prevent duplicate ID error
    let id: RecipeId = format!("{slug}_{index}").into();

    // Slumber doesn't support template methods, so we fill in now
    let rendered_method = request.method.render(variables);

    // The rest parser does not enforce method names
    // It must be checked here
    let method: HttpMethod = rendered_method
        .parse()
        .map_err(|_| anyhow!("Unsupported method: {:?}!", request.method))?;
    let url = try_build_slumber_template(request.url)?;
    let authentication = request.authorization.map(build_authentication);
    let query = build_query(request.query);

    let complete_body = request
        .body
        .map(|b| try_build_body(b, &id, &request.headers, variables));

    let (body, chain) = match complete_body {
        Some(Ok(complete)) => (Some(complete.recipe_body), complete.chain),
        Some(Err(err)) => {
            error!("Failed to convert body! {err}");
            (None, None)
        }
        _ => (None, None),
    };

    let headers = build_slumber_templates(request.headers);

    let recipe = Recipe {
        id,
        persist: true,
        name: name.into(),
        method,
        url,
        authentication,
        body,
        headers,
        query,
    };

    Ok(CompleteRecipe { recipe, chain })
}

/// Rest has no request nesting feature so a tree will always be flat
fn build_recipe_tree_with_chains(
    completed: Vec<CompleteRecipe>,
) -> (RecipeTree, IndexMap<ChainId, Chain>) {
    let mut chains: IndexMap<ChainId, Chain> = IndexMap::new();
    let recipe_node_map = completed
        .into_iter()
        .map(|CompleteRecipe { recipe, chain }| {
            if let Some(load_chain) = chain {
                chains.insert(load_chain.id().clone(), load_chain);
            }

            (recipe.id().clone(), RecipeNode::Recipe(recipe))
        })
        .collect::<IndexMap<RecipeId, RecipeNode>>();

    let recipe_tree = RecipeTree::new(recipe_node_map)
        .expect("IDs are injected by the recipe converter!");

    (recipe_tree, chains)
}

fn flavor_name_and_id(flavor: RestFlavor) -> (String, String) {
    let (name, id) = match flavor {
        RestFlavor::Jetbrains => ("Jetbrains HTTP File", "http_file"),
        RestFlavor::Vscode => ("VSCode Rest File", "rest_file"),
        RestFlavor::Generic => ("Rest File", "rest_file"),
    };
    (name.into(), id.into())
}

/// There is no profile system in Rest,
/// here is a default to use
fn build_profile_map(
    flavor: RestFlavor,
    variables: RestVariables,
) -> IndexMap<ProfileId, Profile> {
    let (flavor_name, flavor_id) = flavor_name_and_id(flavor);
    let profile_id: ProfileId = flavor_id.into();
    let default_profile = Profile {
        id: profile_id.clone(),
        name: Some(flavor_name),
        default: true,
        data: build_slumber_templates(variables),
    };

    IndexMap::from([(profile_id.clone(), default_profile)])
}

fn build_collection(rest_format: RestFormat) -> Collection {
    let RestFormat {
        requests,
        variables,
        flavor,
    } = rest_format;

    let completed_recipes = requests
        .into_iter()
        .enumerate()
        .filter_map(|(index, req)| {
            try_build_recipe(req, index, &variables).traced().ok()
        })
        .collect::<Vec<_>>();

    let (recipes, chains) = build_recipe_tree_with_chains(completed_recipes);

    let profiles = build_profile_map(flavor, variables);

    Collection {
        name: None,
        profiles,
        chains,
        recipes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const REST_FILE: &str = "rest_http_bin.http";
    const REST_IMPORTED_FILE: &str = "rest_imported.yml";

    fn example_vars() -> RestVariables {
        IndexMap::from([
            ("HOST".into(), RestTemplate::new("https://httpbin.org")),
            ("FIRST_NAME".into(), RestTemplate::new("John")),
            ("LAST_NAME".into(), RestTemplate::new("Smith")),
            (
                "FULL_NAME".into(),
                RestTemplate::new("{{FIRST_NAME}} {{LAST_NAME}}"),
            ),
        ])
    }

    /// Catch-all test for REST import
    #[rstest]
    #[tokio::test]
    async fn test_rest_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(REST_FILE));
        let imported = from_rest(&input).await.unwrap();
        let expected =
            Collection::load(&test_data_dir.join(REST_IMPORTED_FILE)).unwrap();
        assert_eq!(imported, expected);
    }

    #[test]
    fn test_convert_basic_request() {
        let test_req = RestRequest {
            name: Some("My Request!".into()),
            url: RestTemplate::new("https://httpbin.org"),
            query: IndexMap::from([
                ("name".into(), RestTemplate::new("joe")),
                ("age".into(), RestTemplate::new("46")),
            ]),
            method: RestTemplate::new("GET"),
            ..RestRequest::default()
        };

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &IndexMap::new()).unwrap();

        assert_eq!(recipe.url, "https://httpbin.org".into());
        assert_eq!(&recipe.query["age"], &"46".into());
        assert_eq!(recipe.method, HttpMethod::Get);
        assert_eq!(recipe.id().clone(), RecipeId::from("My_Request__0"));
    }

    #[test]
    fn test_convert_with_vars() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/get"),
            query: IndexMap::from([
                ("first_name".into(), RestTemplate::new("{{FIRST_NAME}}")),
                ("full_name".into(), RestTemplate::new("{{FULL_NAME}}")),
            ]),
            method: RestTemplate::new("POST"),
            ..RestRequest::default()
        };

        let vars = example_vars();
        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &vars).unwrap();

        assert_eq!(recipe.url, "{{HOST}}/get".into());
        assert_eq!(&recipe.query["first_name"], &"{{FIRST_NAME}}".into());
        assert_eq!(&recipe.query["full_name"], &"{{FULL_NAME}}".into());
        assert_eq!(recipe.method, HttpMethod::Post);
    }

    #[test]
    fn test_fails_on_bad_method() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/get"),
            method: RestTemplate::new("INVALID"),
            ..RestRequest::default()
        };

        let got = try_build_recipe(test_req, 0, &IndexMap::new());
        assert!(got.is_err());
    }

    #[test]
    fn test_build_load_chain() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            headers: IndexMap::from([(
                "Content-Type".into(),
                RestTemplate::new("application/json"),
            )]),
            body: Some(RestBody::LoadFromFile {
                process_variables: false,
                encoding: None,
                filepath: RestTemplate::new("./test_data/rest_pets.json"),
            }),
            ..RestRequest::default()
        };

        let CompleteRecipe { chain, .. } =
            try_build_recipe(test_req, 0, &IndexMap::new()).unwrap();

        let chain = chain.unwrap();
        assert_eq!(chain.id().clone(), ChainId::from("Request_0_body"));
        let expected_source = ChainSource::File {
            path: Template::raw("./test_data/rest_pets.json".into()),
        };
        assert_eq!(chain.source, expected_source);
        assert_eq!(chain.content_type, Some(ContentType::Json));
    }

    #[test]
    fn test_build_raw_body() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            body: Some(RestBody::Text(RestTemplate::new("test data"))),
            ..RestRequest::default()
        };

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &example_vars()).unwrap();

        let body = recipe.body.unwrap();
        assert_eq!(body, RecipeBody::Raw(("test data").into()));
    }

    #[test]
    fn test_build_json_body() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            headers: IndexMap::from([(
                "Content-Type".into(),
                RestTemplate::new("application/json"),
            )]),
            body: Some(RestBody::Text(RestTemplate::new(
                // Template should be transformed correctly
                r#"{"animal": "penguin", "name": "{{ FULL }}"}"#,
            ))),
            ..RestRequest::default()
        };

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &example_vars()).unwrap();

        assert_eq!(
            recipe.body,
            Some(
                RecipeBody::json(
                    json!({"animal": "penguin", "name": "{{FULL}}"})
                )
                .unwrap()
            ),
        );
    }

    /// An invalid JSON body should not be accepted
    #[test]
    fn test_build_json_body_error() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            headers: IndexMap::from([(
                "Content-Type".into(),
                RestTemplate::new("application/json"),
            )]),
            body: Some(RestBody::Text(RestTemplate::new("invalid json"))),
            ..RestRequest::default()
        };

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &example_vars()).unwrap();

        assert_eq!(recipe.body, None,);
    }

    #[test]
    fn test_build_collection_from_rest_format() {
        let test_req_1 = RestRequest {
            name: Some("Query Request".into()),
            url: RestTemplate::new("https://httpbin.org"),
            query: IndexMap::from([
                ("name".into(), RestTemplate::new("joe")),
                ("age".into(), RestTemplate::new("46")),
            ]),
            method: RestTemplate::new("GET"),
            ..RestRequest::default()
        };

        let test_req_2 = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            headers: IndexMap::from([(
                "Content-Type".into(),
                RestTemplate::new("application/json"),
            )]),
            body: Some(RestBody::Text(RestTemplate::new(
                "{\"animal\": \"penguin\"}",
            ))),
            ..RestRequest::default()
        };

        let format = RestFormat {
            requests: vec![test_req_1, test_req_2],
            flavor: RestFlavor::Jetbrains,
            variables: example_vars(),
        };

        let Collection { recipes, .. } = build_collection(format);

        let recipe_1 = recipes.get(&RecipeId::from("Query_Request_0")).unwrap();
        let recipe_2 = recipes.get(&RecipeId::from("Request_1")).unwrap();

        match (recipe_1, recipe_2) {
            (
                RecipeNode::Recipe(Recipe { body: body1, .. }),
                RecipeNode::Recipe(Recipe { body: body2, .. }),
            ) => {
                assert_eq!(body1, &None,);
                assert_eq!(
                    body2,
                    &Some(
                        RecipeBody::json(json!({"animal": "penguin"})).unwrap()
                    )
                );
            }
            _ => panic!("Invalid! {recipe_1:?} {recipe_2:?}"),
        }
    }

    #[test]
    fn test_handle_parse_error() {
        let test_req_1 = RestRequest {
            name: Some("Query Request".into()),
            url: RestTemplate::new("bad template }} for url {{ "),
            query: IndexMap::from([
                ("name".into(), RestTemplate::new("joe")),
                ("age".into(), RestTemplate::new("46")),
            ]),
            method: RestTemplate::new("GET"),
            ..RestRequest::default()
        };

        let vars = example_vars();
        let rec = try_build_recipe(test_req_1, 0, &vars);
        // Should fail to parse this recipe because of bad URL template
        assert!(rec.is_err());

        let test_req_2 = RestRequest {
            name: Some("Query Request".into()),
            url: RestTemplate::new("https://httpbin.org"),
            query: IndexMap::from([
                (
                    "name".into(),
                    RestTemplate::new("bad template {{ for query"),
                ),
                ("age".into(), RestTemplate::new("46")),
            ]),
            method: RestTemplate::new("GET"),
            ..RestRequest::default()
        };

        let vars = example_vars();
        let rec = try_build_recipe(test_req_2, 0, &vars);
        // Should parse and just ignore the invalid query var
        assert!(rec.is_ok());
    }
}
