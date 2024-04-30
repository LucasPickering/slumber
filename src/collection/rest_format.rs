///! Parses a `.rest` or `.http` file
///! These files are used in many IDEs such as Jetbrains, VSCode, and
/// Visual Studio ! Jetbrains and nvim-rest call it `.http`
///! VSCode and Visual Studio call it `.rest`
use anyhow::{anyhow, Context};
use derive_more::FromStr;
use indexmap::{indexmap, IndexMap};
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_until},
    character::complete::{
        alpha1, alphanumeric1, char, newline, space0, space1,
    },
    combinator::{opt, recognize},
    error::Error as NomError,
    multi::many0_count,
    sequence::{pair, tuple},
    IResult, Parser,
};
use reqwest::header::AUTHORIZATION;
use std::{fs::File, io::Read, path::Path, str};
use url::Url;

use crate::template::Template;

use super::{
    jetbrains_env::{JetbrainsEnvImport, JetbrainsEnv}, recipe_tree::RecipeTree, Authentication,
    Collection, Method, Profile, ProfileId, Recipe, RecipeId, RecipeNode,
};

type StrResult<'a> = Result<(&'a str, &'a str), nom::Err<NomError<&'a str>>>;

const REQUEST_DELIMITER: &str = "###";

const REQUEST_NEWLINE: &str = "\r\n";
const BODY_DELIMITER: &str = "\r\n\r\n";

const NAME_ANNOTATION: &str = "@name";

impl Collection {
    /// Convert a jetbrains `.http` file into a slumber a collection
    /// With an optional `http-client.env.json` file in the same directory
    pub fn from_jetbrains(
        jetbrains_file: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let jetbrains_file = jetbrains_file.as_ref();
        has_extension_or_error(jetbrains_file, "http")?;

        Self::from_rest_file(jetbrains_file, None)
    }

    /// Convert a jetbrains `.http` file into a slumber a collection
    /// Including the `http-client.env.json` file in the same directory
    pub fn from_jetbrains_with_public_env(
        jetbrains_file: impl AsRef<Path>
    ) -> anyhow::Result<Self> {
        Self::from_jetbrains_with_env(jetbrains_file, JetbrainsEnvImport::Public)
    }

    /// Convert a jetbrains `.http` file into a slumber a collection
    /// Including the `http-client.env.json` and `http-client.private.env.json` files in the same directory
    pub fn from_jetbrains_with_public_and_private_env(
        jetbrains_file: impl AsRef<Path>
    ) -> anyhow::Result<Self> {
        Self::from_jetbrains_with_env(jetbrains_file, JetbrainsEnvImport::PublicAndPrivate)
    } 

    fn from_jetbrains_with_env(
        jetbrains_file: impl AsRef<Path>,
        import_type: JetbrainsEnvImport 
    ) -> anyhow::Result<Self> {
        let jetbrains_file = jetbrains_file.as_ref();
        has_extension_or_error(jetbrains_file, "http")?;

        let dir = jetbrains_file
            .parent()
            .ok_or(anyhow!("Could not find directory"))?;
        let env = JetbrainsEnv::from_directory(dir, import_type)?;

        Self::from_rest_file(jetbrains_file, Some(env))
    }

    /// Convert a vscode `.rest` file into a slumber a collection
    pub fn from_vscode(vscode_file: impl AsRef<Path>) -> anyhow::Result<Self> {
        let vscode_file = vscode_file.as_ref();
        has_extension_or_error(vscode_file, "rest")?;
        Self::from_rest_file(vscode_file, None)
    }

    /// Convert an `.http` or a `.rest` file into a slumber collection
    fn from_rest_file(
        rest_file: &Path,
        env_file: Option<JetbrainsEnv>,
    ) -> anyhow::Result<Self> {
        let mut file = File::open(rest_file)
            .context(format!("Error opening REST file {rest_file:?}"))?;

        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(format!("Error reading REST file {rest_file:?}"))?;

        let RestFormat { recipes, variables } = RestFormat::from_str(&text)?;
        let tree = build_recipe_tree(recipes)?;

        let profiles = match env_file {
            Some(env) => env.to_profiles(variables)?,
            None => build_default_profiles(variables),
        };

        let collection = Self {
            profiles,
            chains: IndexMap::new(),
            recipes: tree,
            _ignore: serde::de::IgnoredAny,
        };

        Ok(collection)
    }
}

/// If there are no env files, just throw the global variables into a default
/// profile
fn build_default_profiles(
    data: IndexMap<String, Template>,
) -> IndexMap<ProfileId, Profile> {
    indexmap! {
        "default".into() => Profile {
            id: "default".into(),
            name: None,
            data,
        }
    }
}

/// A list of ungrouped recipes is returned from the parser
/// This converts them into a recipe tree
fn build_recipe_tree(recipes: Vec<Recipe>) -> anyhow::Result<RecipeTree> {
    let mut tree: IndexMap<RecipeId, RecipeNode> = IndexMap::new();
    for recipe in recipes.into_iter() {
        let id = recipe.id.clone();
        tree.insert(id, RecipeNode::Recipe(recipe));
    }

    Ok(RecipeTree::new(tree).expect("IDs are generated by index"))
}

/// The different import targets have different extensions
/// `http` or `rest`
fn has_extension_or_error(path: &Path, ext: &str) -> anyhow::Result<()> {
    match path.extension() {
        Some(file_ext) if file_ext != ext => {
            Err(anyhow!("File must have extension \"{}\"", ext))
        }
        None => Err(anyhow!("File must have extension \"{}\"", ext)),
        _ => Ok(()),
    }
}

impl Recipe {
    /// Create a recipe from an optionally named raw http request
    /// Index is needed so an ID can be constructred
    fn from_raw_request(
        name: Option<String>,
        raw_request: &str,
        index: usize,
    ) -> anyhow::Result<Self> {
        let (req_portion, body_portion) =
            parse_request_and_body(raw_request.trim());

        // We need an empty buffer of headers (max of 64)
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);
        let req_buffer = req_portion.as_bytes();
        req.parse(req_buffer).map_err(|parse_err| {
            anyhow!("Failed to parse request! {parse_err:?}")
        })?;

        let path = req
            .path
            .ok_or(anyhow!("There is no path for this request!"))?;

        let RestUrl { url, query } = RestUrl::from_str(path)?;
        let (headers, authentication) = build_headers(req.headers)?;

        let method_literal = req.method.unwrap_or("GET");
        let method = Method::from_str(&method_literal)?;

        let body: Option<Template> = match body_portion {
            Some(raw_body) => Some(raw_body.try_into()?),
            None => None,
        };

        let id_name = format!("request_{}", index + 1);
        let id: RecipeId = id_name.into();
        Ok(Self {
            id,
            name,
            method,
            url,
            body,
            query,
            headers,
            authentication,
        })
    }
}

/// `httparse` doesn't take ownership of the headers
/// This is just coercing them into templates
/// If an authentication header can be found and parsed,
/// turn it into an Authentication struct
fn build_headers(
    headers_slice: &mut [httparse::Header],
) -> anyhow::Result<(IndexMap<String, Template>, Option<Authentication>)> {
    let headers_vec: Vec<httparse::Header> = headers_slice
        .iter()
        .take_while(|h| !h.name.is_empty() && !h.value.is_empty())
        .map(|h| h.to_owned())
        .collect();

    let mut headers: IndexMap<String, Template> = IndexMap::new();
    let mut authentication: Option<Authentication> = None;
    for header in headers_vec {
        let name = header.name.to_string();
        let str_val = str::from_utf8(header.value)
            .context(format!("Cannot parse header {} as UTF8", name))?;

        // If successfully parse authentication from header, save it
        // If it can't be parsed, it will be included as a normal header
        if name.to_lowercase() == AUTHORIZATION.to_string() {
            if let Ok(auth) = Authentication::from_header(str_val) {
                authentication = Some(auth);
                continue;
            }
        }

        let value: Template = str_val
            .to_string()
            .try_into()
            .context(format!("Cannot parse header value as template"))?;
        headers.insert(name, value);
    }
    Ok((headers, authentication))
}

#[derive(Debug, Clone)]
struct RestUrl {
    url: Template,
    query: IndexMap<String, Template>,
}

/// Parse the query portion of a URL
///
/// This injects the query portion into a fake url
/// The template literals in the url would screw up parsing
/// I'd rather use a well tested crate than implementing query parsing
/// There's no public interface in URL to parse the query portion alone
fn parse_query(
    query_portion: &str,
) -> anyhow::Result<IndexMap<String, Template>> {
    let fake_url = Url::parse(&format!("http://localhost?{query_portion}"))
        .context(format!("Invalid query (Query: {query_portion})"))?;

    let mut query: IndexMap<String, Template> = IndexMap::new();
    for (k, v) in fake_url.query_pairs() {
        let key = k.to_string();
        let value: Template = v
            .to_string()
            .try_into()
            .context("Failed to parse query as template (Query: {k}={v})")?;
        query.insert(key, value);
    }
    Ok(query)
}

impl FromStr for RestUrl {
    type Err = anyhow::Error;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        fn url_and_query(input: &str) -> StrResult {
            let (query, (url, _)) = pair(take_until("?"), tag("?"))(input)?;
            Ok((url, query))
        }

        if let Ok((url_part, query_part)) = url_and_query(path) {
            let url: Template = url_part
                .to_string()
                .try_into()
                .context("Failed to parse URL as a template")?;

            let query = parse_query(query_part)?;

            return Ok(Self { url, query });
        }

        // The url is just a string or template
        Ok(Self {
            url: path.to_string().try_into()?,
            query: IndexMap::new(),
        })
    }
}

/// A basic representaion of the REST format
#[derive(Debug, Clone)]
pub struct RestFormat {
    /// A list of recipes
    pub recipes: Vec<Recipe>,
    /// Variables used for templating
    pub variables: IndexMap<String, Template>,
}

/// `httparse` does not parse bodies
/// We need to seperate them from the request portion
fn parse_request_and_body(input: &str) -> (String, Option<String>) {
    fn take_until_body(raw: &str) -> StrResult {
        take_until(BODY_DELIMITER)(raw)
    }

    match take_until_body(input) {
        Ok((body_portion, req_portion)) => {
            let req_with_end = format!("{req_portion}{REQUEST_NEWLINE}");
            (req_with_end, Some(body_portion.trim().into()))
        }
        _ => (input.into(), None),
    }
}

/// A single line during parsing
/// This is the equivalent of a lex token
#[derive(Debug, Clone, PartialEq)]
enum Line {
    /// A section seperator:
    /// `### ?RequestName`
    Seperator(Option<String>),
    /// A request name annotation:
    /// `# @name RequestName`
    Name(String),
    /// A single line of a request:
    /// `POST https://example.com HTTP/1.1`
    Request(String),
}

/// Attempt to parse an optionally named seperator
/// `### {optional_name}`
fn parse_seperator(input: &str) -> IResult<&str, Option<String>> {
    let (input, _) = tag(REQUEST_DELIMITER)(input)?;
    let (input, req_name) =
        opt(pair(space1, take_till(|c| c == ' ' || c == '\n')))(input)?;

    let potential_name = req_name.map(|(_, name)| name.to_string());
    Ok((input, potential_name))
}

/// Attempt to parse a name annotation
/// `# @name RequestName`
fn parse_request_name_annotation(input: &str) -> IResult<&str, &str> {
    let (input, _) = pair(char('#'), space0)(input)?;
    let (input, _) = tag(NAME_ANNOTATION)(input)?;
    let (input, _) = pair(alt((char('='), char(' '))), space0)(input)?;
    let (input, req_name) = take_till(|c| c == ' ' || c == '\n')(input)?;

    Ok((input, req_name.into()))
}

/// Parses an HTTP File variable
/// `@my_variable = hello`
fn parse_variable_assignment(input: &str) -> IResult<&str, (&str, &str)> {
    fn parse_variable_identifier(input: &str) -> IResult<&str, &str> {
        recognize(pair(
            alpha1,
            many0_count(alt((alphanumeric1, tag("_"), tag("-"), tag(".")))),
        ))
        .parse(input)
    }

    let (input, _) = char('@')(input)?;
    let (input, id) = parse_variable_identifier(input)?;

    let (input, _) = tuple((opt(space1), char('='), opt(space1)))(input)?;
    let (input, value) = take_till(|c| c == '\n')(input)?;
    let (input, _) = newline(input)?;

    Ok((input, (id.into(), value.into())))
}

/// Attempt to remove a comment, if one exists
fn parse_line_without_comment(line: &str) -> StrResult {
    fn starting_slash_comment(line: &str) -> StrResult {
        tag("//")(line)
    }

    // A comment can start with `//` but it can't be in the middle
    // This would prevent you from writing urls: `https://`
    if let Ok((inp, _)) = starting_slash_comment(line) {
        return Ok((inp, ""));
    }

    // Hash comments can appear anywhere
    // `GET example.com HTTP/v1.1 # Sends a get request`
    take_until("#")(line)
}

/// Parse an input string line by line
fn parse_lines(
    input: &str,
) -> anyhow::Result<(Vec<Line>, IndexMap<String, Template>)> {
    let mut lines: Vec<Line> = vec![];
    let mut variables: IndexMap<String, Template> = IndexMap::new();
    for line in input.trim().lines() {
        let line = &format!("{line}\n");
        if let Ok((_, seperator_name)) = parse_seperator(line) {
            lines.push(Line::Seperator(seperator_name));
            continue;
        }

        if let Ok((_, name)) = parse_request_name_annotation(line) {
            lines.push(Line::Name(name.into()));
            continue;
        }

        // Now that all the things that look like comments have been parsed,
        // we can remove the comments
        let line = parse_line_without_comment(line)
            .map(|(_, without_comment)| without_comment)
            .unwrap_or(line);

        if let Ok((_, (key, val))) = parse_variable_assignment(line) {
            let value_template: Template = val.to_string().try_into()?;
            variables.insert(key.into(), value_template);
            continue;
        }

        lines.push(Line::Request(line.trim().into()));
    }
    Ok((lines, variables))
}

impl RestFormat {
    /// Take each parsed line (like a lex token) and
    /// convert it to the REST format
    fn from_lines(
        lines: Vec<Line>,
        variables: IndexMap<String, Template>,
    ) -> anyhow::Result<Self> {
        let mut recipes: Vec<Recipe> = vec![];
        let mut current_name: Option<String> = None;
        let mut current_request: String = "".into();
        for line in lines {
            match line {
                Line::Seperator(name_opt) => {
                    if current_request.trim() != "" {
                        let recipe = Recipe::from_raw_request(
                            current_name,
                            &current_request,
                            recipes.len(),
                        )?;
                        recipes.push(recipe);
                    }

                    current_name = None;
                    current_request = "".into();

                    if let Some(name) = name_opt {
                        current_name = Some(name);
                    }
                }
                Line::Name(name) => {
                    current_name = Some(name);
                }
                Line::Request(req) => {
                    let next_line = format!("{req}{REQUEST_NEWLINE}");
                    current_request.push_str(&next_line);
                }
            }
        }

        let recipe = Recipe::from_raw_request(
            current_name,
            &current_request,
            recipes.len(),
        )?;
        recipes.push(recipe);

        Ok(Self { recipes, variables })
    }
}

impl FromStr for RestFormat {
    type Err = anyhow::Error;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (lines, variables) = parse_lines(text)?;
        Ok(Self::from_lines(lines, variables)?)
    }
}

#[cfg(test)]
mod test {
    use crate::collection::CollectionFile;

    use super::*;

    const JETBRAINS_FILE: &str = "./test_data/jetbrains.http";
    const JETBRAINS_RESULT: &str = "./test_data/jetbrains_imported.yml";
    const JETBRAINS_RESULT_WITH_ENV: &str =
        "./test_data/jetbrains_with_env_imported.yml";

    #[tokio::test]
    async fn test_jetbrains_import() {
        let imported = Collection::from_jetbrains(JETBRAINS_FILE).unwrap();
        let expected = CollectionFile::load(JETBRAINS_RESULT.into())
            .await
            .unwrap()
            .collection;
        assert_eq!(imported, expected);
    }

    #[tokio::test]
    async fn test_jetbrains_with_env_import() {
        let imported =
            Collection::from_jetbrains_with_public_and_private_env(JETBRAINS_FILE).unwrap();
        let expected = CollectionFile::load(JETBRAINS_RESULT_WITH_ENV.into())
            .await
            .unwrap()
            .collection;
        assert_eq!(imported, expected);
    }

    #[test]
    fn parse_http_variable() {
        let example_var = "@MY_VAR    = 1231\n";
        let (_, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("MY_VAR", "1231"));

        let example_var = "@MY_NAME =hello\n";
        let (rest, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("MY_NAME", "hello"));
        assert_eq!(rest, "");

        let example_var = "@Cool-Word = super_cool\n";
        let (_, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("Cool-Word", "super_cool"));
    }

    #[test]
    fn parse_seperator_line() {
        let line = "### RequestName";
        let (_, name_opt) = parse_seperator(line).unwrap();
        assert_eq!(name_opt, Some("RequestName".into()));

        let line = "#######";
        let (_, name_opt) = parse_seperator(line).unwrap();
        assert_eq!(name_opt, None);

        let line = "###";
        let (_, name_opt) = parse_seperator(line).unwrap();
        assert_eq!(name_opt, None);

        let line = "#";
        let res = parse_seperator(line);
        assert!(res.is_err());
    }

    #[test]
    fn parse_request_name_test() {
        let line = "# @name=hello";
        let (_, name) = parse_request_name_annotation(line).unwrap();
        assert_eq!(name, "hello".to_string());

        let line = "# @name Cool";
        let (_, name) = parse_request_name_annotation(line).unwrap();
        assert_eq!(name, "Cool".to_string());

        let line = "# a comment";
        assert!(parse_request_name_annotation(line).is_err());
    }

    #[test]
    fn parse_url_test() {
        let example = "{{VAR}}?x={{b}}&word=cool";
        let parsed = RestUrl::from_str(example).unwrap();
        assert_eq!(parsed.url.to_string(), "{{VAR}}");
        assert_eq!(parsed.query.get("x").unwrap().to_string(), "{{b}}");
        assert_eq!(parsed.query.get("word").unwrap().to_string(), "cool");

        let example = "https://example.com";
        let parsed: RestUrl = example.parse().unwrap();
        assert_eq!(parsed.url.to_string(), "https://example.com");
        assert_eq!(parsed.query.len(), 0);

        let example = "https://example.com?q={{query}}";
        let parsed: RestUrl = example.parse().unwrap();
        assert_eq!(parsed.url.to_string(), "https://example.com");
        assert_eq!(parsed.query.get("q").unwrap().to_string(), "{{query}}");

        let example = "{{my_url}}";
        let parsed: RestUrl = example.parse().unwrap();
        assert_eq!(parsed.url.to_string(), "{{my_url}}");
    }

    #[test]
    fn parse_request_and_body_test() {
        let example = r#"
POST /post?q=hello HTTP/1.1
Host: localhost
Content-Type: application/json
X-Http-Method-Override: PUT

{
    "data": "my data"
}
"#
        .trim()
        .replace("\n", REQUEST_NEWLINE);

        let (req, body) = parse_request_and_body(&example);

        assert_eq!(
            req,
            r#"POST /post?q=hello HTTP/1.1
Host: localhost
Content-Type: application/json
X-Http-Method-Override: PUT
"#
            .replace("\n", "\r\n")
        );

        assert_eq!(
            body,
            Some(
                r#"{
    "data": "my data"
}"#
                .replace("\n", "\r\n")
            )
        );
    }
}
