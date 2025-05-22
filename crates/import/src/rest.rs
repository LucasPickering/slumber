//! Import request collections from VSCode `.rest` files or Jetbrains `.http`
//! files. VSCode: https://github.com/Huachao/vscode-restclient
//! Jetbrains: https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html

use anyhow::anyhow;
use indexmap::IndexMap;
use itertools::Itertools;
use serde::de::IgnoredAny;
use slumber_core::{
    collection::{
        Authentication, Chain, ChainId, ChainOutputTrim, ChainSource,
        Collection, HasId, Profile, ProfileId, Recipe, RecipeBody, RecipeId,
        RecipeNode, RecipeTree, SelectorMode,
    },
    http::{HttpMethod, content_type::ContentType},
    template::{Identifier, Template},
};

use reqwest::header;
use rest_parser::{
    Body as RestBody, RestFlavor, RestFormat, RestRequest, RestVariables,
    headers::Authorization as RestAuthorization,
    template::{Template as RestTemplate, TemplatePart as RestTemplatePart},
};
use slumber_util::ResultTraced;
use std::path::Path;
use tracing::error;

/// Convert a VSCode `.rest` file or a Jetbrains `.http` file into a slumber
/// collection
pub fn from_rest(rest_file: impl AsRef<Path>) -> anyhow::Result<Collection> {
    let rest_file = rest_file.as_ref();
    // Parse the file and determine the flavor using the extension
    let rest_format = RestFormat::parse_file(rest_file)?;
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
    // Rest templates allow spaces in variables
    // For example `{{ HOST}}` or `{{ HOST }}`
    // These must be removed before putting it through the slumber
    // template parser
    let raw_template = template
        .parts
        .into_iter()
        .map(|part| match part {
            RestTemplatePart::Text(text) => text,
            RestTemplatePart::Variable(var) => {
                "{{".to_string() + var.as_str() + "}}"
            }
        })
        .join("");

    raw_template
        .parse()
        .map_err(|err| anyhow!("Failed to parse REST template! {err}"))
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

    let content_type =
        guess_is_json(headers, variables).then_some(ContentType::Json);

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

/// Attempt to use headers to determine if the request is JSON
fn guess_is_json(
    r_headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> bool {
    r_headers.iter().any(|(name, value)| {
        name.to_lowercase() == header::CONTENT_TYPE.as_str()
            && value.render(variables) == mime::APPLICATION_JSON.to_string()
    })
}

/// If the request has JSON headers, mark it as such
fn guess_content_type(
    headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> Option<ContentType> {
    guess_is_json(headers, variables).then_some(ContentType::Json)
}

fn try_build_body(
    body: RestBody,
    recipe_id: &str,
    headers: &IndexMap<String, RestTemplate>,
    variables: &RestVariables,
) -> anyhow::Result<CompleteBody> {
    // We only want the text for now
    let (template, chain, content_type) = match body {
        RestBody::Text(text) => (
            try_build_slumber_template(text)?,
            None,
            guess_content_type(headers, variables),
        ),
        RestBody::SaveToFile { text, .. } => {
            (try_build_slumber_template(text)?, None, None)
        }
        RestBody::LoadFromFile { filepath, .. } => {
            let chain = try_build_chain_from_load_body(
                filepath, recipe_id, headers, variables,
            )?;
            let template = Template::from_chain(chain.id().clone());
            (template, Some(chain), None)
        }
    };

    let recipe_body = RecipeBody::Raw {
        body: template,
        content_type,
    };

    Ok(CompleteBody { recipe_body, chain })
}

/// Build the query variables
/// Logging and skipping any invalid templates
fn build_query(
    r_query: IndexMap<String, RestTemplate>,
) -> Vec<(String, Template)> {
    r_query
        .into_iter()
        .filter_map(|(k, v)| {
            try_build_slumber_template(v).map(|t| (k, t)).traced().ok()
        })
        .collect()
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
        profiles,
        chains,
        recipes,
        _ignore: IgnoredAny,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use slumber_util::test_data_dir;

    use super::*;

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

    #[test]
    fn can_convert_basic_request() {
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
        assert_eq!(&recipe.query[1], &("age".into(), "46".into()));
        assert_eq!(recipe.method, HttpMethod::Get);
        assert_eq!(recipe.id().clone(), RecipeId::from("My_Request__0"));
    }

    #[test]
    fn can_convert_with_vars() {
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
        assert_eq!(
            &recipe.query[0],
            &("first_name".into(), ("{{FIRST_NAME}}").into())
        );
        assert_eq!(
            &recipe.query[1],
            &("full_name".into(), ("{{FULL_NAME}}").into())
        );
        assert_eq!(recipe.method, HttpMethod::Post);
    }

    #[test]
    fn fails_on_bad_method() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/get"),
            method: RestTemplate::new("INVALID"),
            ..RestRequest::default()
        };

        let got = try_build_recipe(test_req, 0, &IndexMap::new());
        assert!(got.is_err());
    }

    #[test]
    fn can_build_load_chain() {
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
    fn can_build_raw_body() {
        let test_req = RestRequest {
            url: RestTemplate::new("{{HOST}}/post"),
            method: RestTemplate::new("POST"),
            body: Some(RestBody::Text(RestTemplate::new("test data"))),
            ..RestRequest::default()
        };

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &example_vars()).unwrap();

        let body = recipe.body.unwrap();
        assert_eq!(
            body,
            RecipeBody::Raw {
                body: ("test data").into(),
                content_type: None,
            }
        );
    }

    #[test]
    fn can_build_json_body() {
        let test_req = RestRequest {
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

        let CompleteRecipe { recipe, .. } =
            try_build_recipe(test_req, 0, &example_vars()).unwrap();

        let body = recipe.body.unwrap();
        assert_eq!(
            body,
            RecipeBody::Raw {
                body: ("{\"animal\": \"penguin\"}").into(),
                content_type: Some(ContentType::Json),
            }
        );
    }

    #[test]
    fn can_build_collection_from_rest_format() {
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
                    &Some(RecipeBody::Raw {
                        body: ("{\"animal\": \"penguin\"}").into(),
                        content_type: Some(ContentType::Json),
                    })
                );
            }
            _ => panic!("Invalid! {recipe_1:?} {recipe_2:?}"),
        }
    }

    fn remove_whitespace(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn can_handle_parse_error() {
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

    #[test]
    fn can_load_collection_from_file() {
        let test_http_path = test_data_dir().join("rest_http_bin.http");
        let test_slumber_path = test_data_dir().join("rest_imported.yml");

        let collection = from_rest(test_http_path).unwrap();
        let loaded_collection = Collection::load(&test_slumber_path).unwrap();

        assert_eq!(collection.profiles, loaded_collection.profiles);
        assert_eq!(collection.chains, loaded_collection.chains);

        // Saving and loading messes with the JSON whitespace
        // Compare it here

        let recipe_1 = RecipeId::from("SimpleGet_0");
        let recipe_2 = RecipeId::from("JsonPost_1");
        let recipe_3 = RecipeId::from("Request_2");
        let recipe_4 = RecipeId::from("Pet_json_3");
        assert_eq!(
            collection.recipes.try_get_recipe(&recipe_1).unwrap(),
            loaded_collection.recipes.try_get_recipe(&recipe_1).unwrap()
        );
        assert_eq!(
            collection.recipes.try_get_recipe(&recipe_3).unwrap(),
            loaded_collection.recipes.try_get_recipe(&recipe_3).unwrap()
        );
        assert_eq!(
            collection.recipes.try_get_recipe(&recipe_4).unwrap(),
            loaded_collection.recipes.try_get_recipe(&recipe_4).unwrap()
        );

        // This request should have slightly different whitespace because it is
        // JSON parsed To avoid rendering the output, just clean up the
        // debug output and compare that
        let bod_1 = collection.recipes.try_get_recipe(&recipe_2).unwrap();
        let bod_2 = collection.recipes.try_get_recipe(&recipe_2).unwrap();

        match (bod_1, bod_2) {
            (
                Recipe {
                    body: Some(RecipeBody::Raw { body: b1, .. }),
                    ..
                },
                Recipe {
                    body: Some(RecipeBody::Raw { body: b2, .. }),
                    ..
                },
            ) => {
                let deb_1 = remove_whitespace(&format!("{b1:?}"));
                let deb_2 = remove_whitespace(&format!("{b2:?}"));
                assert_eq!(deb_1, deb_2);
            }
            _ => panic!("Invalid Json"),
        }
    }
}
