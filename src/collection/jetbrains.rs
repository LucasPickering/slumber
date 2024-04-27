use derive_more::FromStr;
use nom::{
    branch::alt, bytes::complete::{tag, take_till, take_until}, character::{complete::{alpha1,alphanumeric1, anychar, char, multispace0, multispace1, newline, one_of, space0, space1}, is_space}, combinator::{opt, recognize}, error::{Error as NomError, ErrorKind, ParseError, VerboseError}, multi::many0_count, sequence::{delimited, pair, tuple}, FindSubstring, Finish, IResult, InputLength, InputTake, Offset, Parser
};

use crate::template::parser_utils::{take_until_or_eof, ParseResult};

/// Notes:
/// for line in lines:
///     parse_separator: check for ###, check if name exists `### RequestName`
///     parse name annotation: m match `# @name=Name` or `# @name Name`
///     parse_comment: `hello hello // comment`, `hello # comment`, `// comment`, '# comment'
///     parse_variable: @x = y
///     parse_request: build up the request line by line, fill in the variables 


type StrResult<'a> = Result<(&'a str, &'a str), nom::Err<NomError<&'a str>>>;

const REQUEST_DELIMITER: &str = "###";
const NAME_ANNOTATION: &str = "@name";

type HttpVariable = (String, String);

#[derive(Debug, Clone, PartialEq)]
enum Line {
    Seperator,
    Name(String),
    Variable(String, String),
    Request(String),
}

fn parse_seperator(input: &str) -> IResult<&str, Vec<Line>> {
    let (input, _) = tag(REQUEST_DELIMITER)(input)?;
    let (input, req_name) = opt(pair(
        space1,
        take_till(|c| c == ' ' || c == '\n')
    ))(input)?;

    let got = match req_name {
        Some((_, name)) => vec![Line::Seperator, Line::Name(name.into())],
        None => vec![Line::Seperator],
    };
    Ok((input, got))
}

fn parse_request_name(input: &str) -> IResult<&str, Line> {
    let (input, _) = pair(tag("#"), space0)(input)?;
    let (input, _) = tag(NAME_ANNOTATION)(input)?;
    let (input, _) = pair(alt((tag("="), tag(" "))), space0)(input)?;
    let (input, req_name) = take_till(|c| c == ' ' || c == '\n')(input)?; 

    Ok((input, Line::Name(req_name.into())))
}

/// Parses an HTTP File variable (@MY_VAR = 1234)
fn parse_variable(input: &str) -> IResult<&str, Line> {
    let (input, _) = char('@')(input)?;
    let (input, name) = recognize(pair(
        alpha1,
        many0_count(alt((alphanumeric1, tag("_"), tag("-"), tag("."))))
    )).parse(input)?;

    let (input, _) = tuple((opt(space1), char('='), opt(space1)))(input)?;
    let (input, value) = take_till(|c| c == '\n')(input)?;
    let (input, _) = newline(input)?;

    Ok((input, Line::Variable(name.into(), value.into())))
}


fn parse_line_without_comment(line: &str) -> StrResult {
    alt((take_until("#"), take_until("//")))(line)
}

// fn parse_request_line(line: &str) -> IResult<&str, Line> {
//     
// } 


fn parse(input: &str) {
    // for line in input.lines() {
    //     // let (c, _) = line_without_comment(line)


    // }


}


#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_http_variable() {
        let example_var = "@MY_VAR    = 1231\n";
        let (_, var) = parse_variable(example_var).unwrap();

        assert_eq!(
            var,
            Line::Variable("MY_VAR".into(), "1231".into()),
        );

        let example_var = "@MY_NAME =hello\n";
        let (rest, var) = parse_variable(example_var).unwrap();

        assert_eq!(var, Line::Variable("MY_NAME".into(), "hello".into()));
        assert_eq!(rest, "");

        let example_var = "@Cool-Word = super_cool\n";
        let (_, var) = parse_variable(example_var).unwrap();

        assert_eq!(var, Line::Variable("Cool-Word".into(), "super_cool".into()));

        println!("{var:?}");
    }

    #[test]
    fn parse_seperator_line() {
        let line = "### RequestName";
        let (_, items) = parse_seperator(line).unwrap();
        assert_eq!(items, vec![Line::Seperator, Line::Name("RequestName".into())]);

        let line = "#######";
        let (_, items) = parse_seperator(line).unwrap();
        assert_eq!(items, vec![Line::Seperator]);

        let line = "###";
        let (_, items) = parse_seperator(line).unwrap();
        assert_eq!(items, vec![Line::Seperator]);

        let line = "#";
        let res = parse_seperator(line);
        assert!(res.is_err());
    }

    #[test]
    fn parse_request_name_test() {
        let line = "# @name=hello";
        let (_, name) = parse_request_name(line).unwrap();
        assert_eq!(name, Line::Name("hello".into()));  

        let line = "# @name Cool";
        let (_, name) = parse_request_name(line).unwrap();
        assert_eq!(name, Line::Name("Cool".into()));  
    }


    #[test]
    fn extract_http_variables() {
        let example = r#"
@MY_VAR = 123
@hello=blahblah
GET blahblah HTTP/1.1

example example

@var = 12
        "#;
        
        // TODO: 
    }

    #[test]
    fn parse_http_sections() {
        let example_file = r#"
lorem
ipsume
dolor // comment end of line
########
lorem # comment at end of line
ipsum
# comment
// another comment
dolor
###
lorem
        "#;

        // TODO:   
   }

    #[test]
    fn parse_http_file() {
        println!("Hello!");
    }
}
