#![allow(dead_code)]
///! Parses a `.rest` or `.http` file 
///! These files are used in many IDEs such as Jetbrains, VSCode, and Visual Studio
///! Jetbrains and nvim-rest call it `.http` 
///! VSCode and Visual Studio call it `.rest`

use std::io::Read;
use std::{str::Utf8Error};
use std::path::Path;
use std::fs::File;
use std::str;
use derive_more::FromStr;
use nom::{
    branch::alt, bytes::{complete::{tag, take_till, take_until}}, character::complete::{
        alpha1, alphanumeric1, char, 
        newline, space0, space1,
    }, combinator::{opt, recognize}, error::Error as NomError, multi::many0_count, sequence::{pair, tuple}, AsBytes, IResult, Parser
};
use httparse::{Request, Header};
use anyhow::{anyhow, Context};
use url::{Url, UrlQuery};
use indexmap::IndexMap;

use crate::collection::RecipeId;
use crate::template::Template;

use super::{Collection, Method, Profile, ProfileId, Recipe, RecipeNode};
use super::recipe_tree::RecipeTree;

type StrResult<'a> = Result<(&'a str, &'a str), nom::Err<NomError<&'a str>>>;

const REQUEST_DELIMITER: &str = "###";
const BODY_DELIMITER: &str = "\r\n\r\n";

const NAME_ANNOTATION: &str = "@name";

const VARIABLE_OPEN: &str = "{{";
const VARIABLE_CLOSE: &str = "}}";

/// REST is a pretty simple format and doesn't have anything like profiles
/// It does have variables, so this just throws them into a default profile
impl Profile {
    fn default_with_data(data: IndexMap<String, Template>) -> Self {
        Self {
            id: "default".into(),
            name: None,
            data,
        }        
    }
}

impl RecipeTree {
    fn from_recipe_vector(recipes: Vec<Recipe>) -> anyhow::Result<Self> {
        let mut tree: IndexMap<RecipeId, RecipeNode> = IndexMap::new(); 
        for recipe in recipes.into_iter() {
            let id = recipe.id.clone(); 
            tree.insert(id, RecipeNode::Recipe(recipe));
        }

        Ok(RecipeTree::new(tree).unwrap())
    }
}

fn has_extension_or_error(path: &Path, ext: &str) -> anyhow::Result<()> {
    match path.extension() {
        Some(file_ext) if file_ext != ext => Err(anyhow!("File must have extension \"{}\"", ext)),
        None => Err(anyhow!("File must an extension")),
        _ => Ok(())
    }
}

impl Collection {
    pub fn from_jetbrains(
        jetbrains_file: impl AsRef<Path>
    ) -> anyhow::Result<Self> {
        let jetbrains_file = jetbrains_file.as_ref();
        has_extension_or_error(jetbrains_file, "http")?; 
        Self::from_rest_file(jetbrains_file)
    }

    pub fn from_vscode(
        vscode_file: impl AsRef<Path>
    ) -> anyhow::Result<Self> {
        let vscode_file = vscode_file.as_ref();
        has_extension_or_error(vscode_file, "rest")?;
        Self::from_rest_file(vscode_file)
    }
    
    fn from_rest_file(
        rest_file: &Path 
    ) -> anyhow::Result<Self> {
        let mut file = File::open(rest_file).context(format!(
            "Error opening REST file {rest_file:?}"
        ))?;

        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(format!("Error reading REST file {rest_file:?}"))?;

        Self::from_rest_str(&text)
    }


    fn from_rest_str(text: &str) -> anyhow::Result<Collection> {
        let RestFormat { recipes, variables } = RestFormat::from_str(&text)?;
        let tree = RecipeTree::from_recipe_vector(recipes)?; 
        let default_profile = Profile::default_with_data(variables);
       
        let profile_id = default_profile.id.clone();
        let profiles = IndexMap::from([
            (profile_id, default_profile)
        ]);

        let collection = Self {
            profiles,
            chains: IndexMap::new(),
            recipes: tree, 
            _ignore: serde::de::IgnoredAny,
        };

        Ok(collection)
    }
}


impl Recipe {
    fn from_raw_rest(name: Option<String>, raw_request: &str, index: usize) -> anyhow::Result<Self> {
        let (req_portion, body_portion) = parse_request_and_body(raw_request.trim()); 
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);
        let req_buffer = req_portion.as_bytes();
        req.parse(req_buffer)
            .map_err(|_| anyhow!("Failed to parse request!"))?;

        let path = req.path
            .ok_or(anyhow!("There is no path for this request!"))?;
        let RestUrl { url, query } = RestUrl::from_str(path)?;
        let headers = build_header_map(req.headers)?;

        let method_literal = req.method
            .ok_or(anyhow!("There is not a method for this request!"))?;
        let method = Method::from_str(&method_literal)?;

        let body: Option<Template> = match body_portion {
            Some(raw_body) => Some(raw_body.try_into()?),
            None => None,
        };

        let id_name = format!("request_{}", index+1);
        let id: RecipeId = id_name.into();
        let recipe = Self {
            id,
            name,
            method,
            url,
            body,
            query,
            headers,
            authentication: None,
        };
        println!("{:?}\n\n", recipe);

        Ok(recipe)
    }

}

/// `httparse` doesn't take ownership of the headers
/// This is just coercing them into an easier format
fn build_header_map(headers_slice: &mut [Header]) -> anyhow::Result<IndexMap<String, Template>> {
    let headers_vec: Vec<Header> = headers_slice
        .iter()
        .take_while(|h| !h.name.is_empty() && !h.value.is_empty())
        .map(|h| h.to_owned())
        .collect();

    let mut headers: IndexMap<String, Template> = IndexMap::new();
    for header in headers_vec {
        let name = header.name.to_string();
        let str_val = str::from_utf8(header.value)?;
        let value: Template = str_val.to_string().try_into()?;
        headers.insert(name, value);
    }

    Ok(headers)
}

#[derive(Debug, Clone)]
struct RestUrl {
    url: Template,
    query: IndexMap<String, Template>,
}

impl FromStr for RestUrl {
    type Err = anyhow::Error;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        if path.contains("?") {
            let mut parts = path.split("?");
            let url_part = parts.next()
                .ok_or(anyhow!("Invalid url"))?
                .to_string();
            let query_part = parts.next()
                .ok_or(anyhow!("Invalid query"))?;

            let url: Template = url_part.try_into()?;
            
            // Inject the query into a localhost url
            // The template literals in the url would screw up parsing
            // I'd rather use a well tested crate that implementing query parsing
            let fake_url = Url::parse(&format!("http://localhost?{query_part}"))?;
            
            let mut query: IndexMap<String, Template> = IndexMap::new();
            for (k, v) in fake_url.query_pairs() {
                let key = k.to_string();
                let value: Template = v.to_string().try_into()?;
                query.insert(key, value);
            }

            return Ok(Self {
                url,
                query,
            })
        }
        
        Ok(Self {
            url: path.to_string().try_into()?,
            query: IndexMap::new(),
        })
    }
}


#[derive(Debug, Clone)]
pub struct RestFormat {
    pub recipes: Vec<Recipe>,
    pub variables: IndexMap<String, Template>,
}

fn parse_request_and_body(input: &str) -> (String, Option<String>) {
    fn take_until_body(raw: &str) -> StrResult {
        take_until(BODY_DELIMITER)(raw)
    }

    match take_until_body(input) { 
        Ok((body_portion, req_portion)) => (req_portion.into(), Some(body_portion.trim().into())),
        _ => (input.into(), None),
    }
} 

/// A single line during parsing
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


fn parse_seperator(input: &str) -> IResult<&str, Option<String>> {
    let (input, _) = tag(REQUEST_DELIMITER)(input)?;
    let (input, req_name) =
        opt(pair(space1, take_till(|c| c == ' ' || c == '\n')))(input)?;

    let potential_name = req_name.map(|(_, name)| name.to_string());
    Ok((input, potential_name))
}

fn parse_request_name_annotation(input: &str) -> IResult<&str, &str> {
    let (input, _) = pair(char('#'), space0)(input)?;
    let (input, _) = tag(NAME_ANNOTATION)(input)?;
    let (input, _) = pair(alt((char('='), char(' '))), space0)(input)?;
    let (input, req_name) = take_till(|c| c == ' ' || c == '\n')(input)?;

    Ok((input, req_name.into()))
}

fn parse_variable_identifier(input: &str) -> IResult<&str, &str> {
    recognize(pair(
        alpha1,
        many0_count(alt((alphanumeric1, tag("_"), tag("-"), tag(".")))),
    ))
    .parse(input)
}

/// Parses an HTTP File variable (@MY_VAR = 1234)
fn parse_variable_assignment(input: &str) -> IResult<&str, (&str, &str)> {
    let (input, _) = char('@')(input)?;
    let (input, id) = parse_variable_identifier(input)?;

    let (input, _) = tuple((opt(space1), char('='), opt(space1)))(input)?;
    let (input, value) = take_till(|c| c == '\n')(input)?;
    let (input, _) = newline(input)?;

    Ok((input, (id.into(), value.into())))
}

fn starting_slash_comment(line: &str) -> StrResult {
    tag("//")(line)
}

fn parse_line_without_comment(line: &str) -> StrResult {
    // A comment can start with `//` but it can't be in the middle
    // This would prevent you from writing urls: `https://`
    if let Ok((inp, _)) = starting_slash_comment(line) {
        return Ok((inp, ""));
    }
    
    // Hash comments can appear anywhere
    // `GET example.com HTTP/v.1 # Sends a get request`
    take_until("#")(line)
}

fn parse_variable_substitution(input: &str) -> StrResult {
    let (input, _) = pair(tag(VARIABLE_OPEN), space0)(input)?;
    let (input, id) = parse_variable_identifier(input)?;
    let (input, _) = pair(space0, tag(VARIABLE_CLOSE))(input)?;

    Ok((input, id))
}

fn until_variable_open(input: &str) -> StrResult {
    take_until(VARIABLE_OPEN)(input)
}

fn parse_lines(input: &str) -> anyhow::Result<(Vec<Line>, IndexMap<String, Template>)> {
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
                        let recipe = Recipe::from_raw_rest(
                            current_name, &current_request, recipes.len()
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
                    current_request.push_str(&format!("{req}\r\n"));
                }
            }
        }

        let request = Recipe::from_raw_rest(current_name, &current_request, recipes.len())?;
        recipes.push(request);

        Ok(Self {
            recipes,
            variables,
        })
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
    use super::*;

    #[test]
    fn parse_http_variable() {
        let example_var = "@MY_VAR    = 1231\n";
        let (_, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("MY_VAR", "1231"),);

        let example_var = "@MY_NAME =hello\n";
        let (rest, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("MY_NAME", "hello"));
        assert_eq!(rest, "");

        let example_var = "@Cool-Word = super_cool\n";
        let (_, var) = parse_variable_assignment(example_var).unwrap();

        assert_eq!(var, ("Cool-Word", "super_cool"));

        println!("{var:?}");
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
    }

    #[test]
    fn parse_lines_test() {
        let example = r#"
###
@MY_VAR = 123
@hello=blahblah
GET https://httpbin1.org HTTP/1.1
Host: localhost

// Comment
@var = 12

### HelloHttpBinRequest

GET {{hello}} HTTP/1.1

example example
######
# @name JSONRequest

POST /post?q=hello HTTP/1.1
Host: localhost
Content-Type: application/json
X-Http-Method-Override: PUT

{
    "data": "my data"
}
        "#;

        let file = RestFormat::from_str(example).unwrap();
        let output = format!("{:?}", file.recipes);
        println!(
            "{}",
            output.replace("JetbrainsRequest {", "\nJetbrainsRequest {")
        );
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
"#.trim().replace("\n", "\r\n");

        let (req, body) = parse_request_and_body(&example);
        println!("{req:?}");
        println!("{body:?}");
    }


    const JETBRAINS_FILE: &str = "./test_data/jetbrains.http";

    #[test]
    fn test_jetbrains_import() {
        let imported = Collection::from_jetbrains(JETBRAINS_FILE).unwrap();
        println!("{:?}", imported); 
    }
}
