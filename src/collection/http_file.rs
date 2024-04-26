use nom::{
    branch::alt,
    bytes::{
        complete::{tag, take_until},
        streaming::take_till,
    },
    character::complete::{alpha1, anychar, char, newline, space1},
    combinator::{all_consuming, eof, opt},
    error::Error as NomError,
    sequence::{delimited, pair, tuple},
    IResult,
};

type StrResult<'a> = Result<(&'a str, &'a str), nom::Err<NomError<&'a str>>>;

#[derive(Debug, Clone, PartialEq)]
struct HttpVariable {
    name: String,
    value: String,
}

/// Parses an HTTP File variable (@MY_VAR = 1234)
fn parse_variable(input: &str) -> IResult<&str, HttpVariable> {
    let (input, _) = char('@')(input)?;
    let (input, (first, rest)) = pair(
        alpha1,
        take_till(|c: char| c != '_' && !c.is_alphanumeric()),
    )(input)?;

    let name: String = format!("{first}{rest}");

    let (input, _) = tuple((opt(space1), char('='), opt(space1)))(input)?;
    let (input, quote_type) = opt(alt((char('\''), char('"'))))(input)?;
    let (input, value) = if let Some(quote) = quote_type {
        take_till(|c| c == quote)(input)?
    } else {
        take_till(|c| c == '\n')(input)?
    };
    let (input, _) = opt(newline)(input)?;

    let value: String = value.into();
    Ok((input, HttpVariable { name, value }))
}

fn parse_delimited_section(
    input: &str,
) -> StrResult { 
    delimited(anychar, take_until("###"), tag("###"))(input)
}

fn parse_sections(input: &str) -> IResult<String, Vec<String>> {
    let mut sections: Vec<String> = vec![];

    let with_suffix = format!("{input}###");
    let mut inp = with_suffix.as_str();
    while let Ok((next_inp, got)) = parse_delimited_section(inp) {
        sections.push(got.into());
        inp = next_inp;
    }

    let sections: Vec<String> =
        sections.into_iter().filter(|s| s != "").collect();
    let sections: Vec<_> = remove_comments(sections);
    Ok((inp.to_string(), sections))
}

fn line_without_comment(
    line: &str,
) -> StrResult { 
    take_until("#")(line)
}

fn remove_comments(sections: Vec<String>) -> Vec<String> {
    sections
        .into_iter()
        .map(|section| {
            section
                .lines()
                .map(|line| {
                    match line_without_comment(line).ok() {
                        Some((_, got)) => got,
                        None => line,
                    }
                })
                .collect::<Vec<&str>>()
                .join("\r\n")
        })
        .collect()
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
            HttpVariable {
                name: "MY_VAR".into(),
                value: "1231".into(),
            }
        );

        let example_var = "@MY_NAME ='hello'\n";
        let (rest, var) = parse_variable(example_var).unwrap();

        assert_eq!(
            var,
            HttpVariable {
                name: "MY_NAME".into(),
                value: "hello".into(),
            }
        );
        assert_eq!(rest, "");

        let example_var = "@WORD= \"super cool\"\n";
        let (_, var) = parse_variable(example_var).unwrap();

        assert_eq!(
            var,
            HttpVariable {
                name: "WORD".into(),
                value: "super cool".into(),
            }
        );

        println!("{var:?}");
    }

    #[test]
    fn parse_http_sections() {
        let example_file = r#"
lorem
ipsume
########
lorem # comment at end of line
ipsum
# comment 
dolor
###
lorem
        "#;

        let sections = parse_sections(&example_file);
        println!("{:?}", sections);
    }

    #[test]
    fn parse_http_file() {
        println!("Hello!");
    }
}
