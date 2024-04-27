#![allow(dead_code)]

use std::collections::HashMap;

use nanohttp::{Header, Method, Path, Request};
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_until},
    character::{
        complete::{
            alpha1, alphanumeric1, anychar, char, multispace0, multispace1,
            newline, one_of, space0, space1,
        },
        is_space,
    },
    combinator::{not, opt, recognize},
    error::{Error as NomError, ErrorKind, ParseError, VerboseError},
    multi::{many0_count, many1},
    sequence::{delimited, pair, tuple},
    Parser, IResult,
};

/// Notes:
/// for line in lines:
///     parse_separator: check for ###, check if name exists `### RequestName`
///     parse name annotation: m match `# @name=Name` or `# @name Name`
///     parse_comment: `hello # comment`, `// comment`, '# comment'
///     parse_variable: @x = y
///     parse_request: build up the request line by line, fill in the variables

type StrResult<'a> = Result<(&'a str, &'a str), nom::Err<NomError<&'a str>>>;

const REQUEST_DELIMITER: &str = "###";
const NAME_ANNOTATION: &str = "@name";

const VARIABLE_OPEN: &str = "{{";
const VARIABLE_CLOSE: &str = "}}";

/// The variable literals are replaced with URL safe characters so `nanohttp` can parse it as
/// normal
const PLACEHOLDER_VARIABLE_OPEN: &str = "VAROPEN";
const PLACEHOLDER_VARIABLE_CLOSE: &str = "VARCLOSE";

#[derive(Debug, Clone, PartialEq)]
enum Line {
    Seperator(Option<String>),
    Name(String),
    Request(String),
}

#[derive(Debug, Clone, PartialEq)]
struct JetbrainsRequest {
    name: Option<String>,
    method: Method,
    path: Path,
    version: String,
    scheme: String,
    headers: Vec<Header>,
}

impl JetbrainsRequest {
    fn from(name: Option<String>, raw_request: &str) -> Self {
        let request = Request::from_string(raw_request).unwrap();
        let headers = request.headers;
        let method = request.method;
        let path = request.path;
        let scheme = request.scheme;
        let version = request.version;
        Self {
            name,
            method,
            path,
            version,
            scheme,
            headers,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct JetbrainsHttp {
    sections: Vec<JetbrainsRequest>,
    variables: HashMap<String, String>,
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
    // A comment can start with `//` but it cant be in the middle
    // This would prevent you from writing urls: `https://`
    if let Ok((inp, _)) = starting_slash_comment(line) {
        return Ok((inp, ""));
    }

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

fn parse_lines(input: &str) -> (Vec<Line>, HashMap<String, String>) {
    let mut lines: Vec<Line> = vec![];
    let mut variables: HashMap<String, String> = HashMap::new();
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
            variables.insert(key.into(), val.into());
            continue;
        }

        if line != "\n" {
            let placeholder_line = line;
                // .replace(VARIABLE_OPEN, PLACEHOLDER_VARIABLE_OPEN)
                // .replace(VARIABLE_CLOSE, PLACEHOLDER_VARIABLE_CLOSE);
            lines.push(Line::Request(placeholder_line.into()));
        }
    }
    (lines, variables)
}

impl JetbrainsHttp {
    fn from_lines(
        lines: Vec<Line>,
        variables: HashMap<String, String>,
    ) -> Self {
        let mut sections: Vec<JetbrainsRequest> = vec![];
        let mut current_name: Option<String> = None;
        let mut current_request: String = "".into();
        for line in lines {
            match line {
                Line::Seperator(name_opt) => {
                    if current_request != "" {
                        let request = JetbrainsRequest::from(
                            current_name,
                            &current_request,
                        );
                        sections.push(request);
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

        let request = JetbrainsRequest::from(current_name, &current_request);
        sections.push(request);

        Self {
            sections,
            variables,
        }
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

POST /post HTTP/1.1
Host: localhost
Content-Type: application/json
X-Http-Method-Override: PUT

{
    "data": "my data"
}
        "#;

        let (lines, variables) = parse_lines(example);
        println!("{:?}", lines);

        let file = JetbrainsHttp::from_lines(lines, variables);
        let output = format!("{:?}", file.sections);
        println!(
            "{}",
            output.replace("JetbrainsRequest {", "\nJetbrainsRequest {")
        );
    }
}
