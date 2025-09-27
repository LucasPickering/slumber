//! Parse Insomnia's complex `{% ... %}` expressions

use base64::{Engine, prelude::BASE64_STANDARD};
use slumber_template::{Expression, Template};
use std::{borrow::Cow, error::Error as StdError};
use tracing::error;
use winnow::{
    ModalParser, ModalResult, Parser,
    ascii::{dec_int, multispace0, multispace1},
    combinator::{alt, delimited, separated, separated_pair},
    error::EmptyError,
    token::take_until,
};

/// Convert an Insomnia template to a Slumber template. If we fail to parse it,
/// just treat it as a literal template.
pub fn parse_template(insomnia: String) -> Template {
    // The error reporting on this is shitty because I'm doing it quickly. I
    // have no idea if anyone actually cares about this so I'm not putting much
    // work into it.
    if let Ok(template) = parse.parse(&insomnia) {
        template
    } else {
        // If it looks like a template but we couldn't parse it, log it
        if insomnia.starts_with("{%") {
            error!("Failed to parse template `{insomnia}`");
        }
        Template::raw(insomnia)
    }
}

/// Build a function call from an Insomnia expression
fn build_call(
    function: &str,
    arguments: Vec<Cow<'_, str>>,
) -> Result<Expression, Box<dyn StdError>> {
    match function {
        "response" => {
            // We expect exactly 5 arguments always
            let [source, request_id, path, trigger, duration] =
                arguments.try_into().map_err(|v: Vec<_>| {
                    format!("Expected 5 arguments, received {}", v.len())
                })?;

            let trigger: Option<Expression> = match &*trigger {
                "never" => None,
                "no-history" => Some("no_history".into()),
                // Duration is always in seconds
                "when-expired" => Some(format!("{duration}s").into()),
                "always" => Some("always".into()),
                _ => {
                    return Err(
                        format!("Unknown request trigger `{trigger}`").into()
                    );
                }
            };
            let expression: Expression = match &*source {
                // Use the whole body
                "raw" => Expression::call(
                    "response",
                    [request_id.into()],
                    [("trigger", trigger)],
                ),
                // Grab the body then use a jsonpath
                "body" => Expression::call(
                    "response",
                    [request_id.into()],
                    [("trigger", trigger)],
                )
                .pipe("jsonpath", [path.into()], []),
                "header" => Expression::call(
                    "response_header",
                    // The path will be the name of the header
                    [request_id.into(), path.into()],
                    [("trigger", trigger)],
                ),
                _ => {
                    return Err(
                        format!("Unknown response source `{source}`").into()
                    );
                }
            };
            Ok(expression)
        }
        _ => Err(format!("Unknown function `{function}`").into()),
    }
}

/// Parse a string like:
/// {% response 'url', 'req_id', 'b64::JC5kYXRh::46b', 'always', 60 %}
fn parse(input: &mut &str) -> ModalResult<Template, EmptyError> {
    ws(delimited(
        "{%",
        ws(separated_pair(
            "response", // This is the only fn we support for now
            multispace1,
            separated(0.., argument, ws(",")),
        )),
        "%}",
    ))
    .try_map(|(function, arguments): (&str, Vec<Cow<'_, str>>)| {
        build_call(function, arguments).map(Template::from)
    })
    .parse_next(input)
}

/// Wrap a parser to allow whitespace on either side of it
fn ws<'a, O, F>(parser: F) -> impl ModalParser<&'a str, O, EmptyError>
where
    F: ModalParser<&'a str, O, EmptyError>,
{
    delimited(multispace0, parser, multispace0)
}

/// Parse an Insomnia argument. If it's a string, remove the wrapping single
/// quotes. If it's a base64 string like `'b64::asdf::46b', remove the
/// head/tail and decode it.
fn argument<'a>(input: &mut &'a str) -> ModalResult<Cow<'a, str>, EmptyError> {
    alt((
        // If we have encoded b64 content, decode it now
        delimited("'b64::", take_until(0.., ":"), "::46b'").try_map(|s| {
            let bytes = BASE64_STANDARD.decode(s)?;
            let content = String::from_utf8(bytes)?;
            Ok::<_, Box<dyn StdError>>(Cow::Owned(content))
        }),
        string.map(Cow::Borrowed),
        dec_int.map(|i: i64| i.to_string().into()),
    ))
    .parse_next(input)
}

/// Parse a string with single quotes
fn string<'a>(input: &mut &'a str) -> ModalResult<&'a str, EmptyError> {
    // Not supporting escaping until I know we need it
    delimited("'", take_until(0.., "'"), "'").parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[rstest]
    #[case::response_body(
        "{% response 'raw', 'req', 'b64::JC5kYXRh::46b', 'never', 60 %}",
        "{{ response('req') }}"
    )]
    #[case::response_body_jsonpath(
        "{% response 'body', 'req', 'b64::JC5kYXRh::46b', 'never', 60 %}",
        "{{ response('req') | jsonpath('$.data') }}"
    )]
    #[case::response_header(
        "{% response 'header', 'req', 'b64::Y29udGVudC10eXBl::46b', 'never', 60 %}",
        "{{ response_header('req', 'content-type') }}"
    )]
    #[case::response_url(
        // This one isn't supported
        "{% response 'url', 'req', 'b64::JC5kYXRh::46b', 'never', 60 %}",
        "{% response 'url', 'req', 'b64::JC5kYXRh::46b', 'never', 60 %}"
    )]
    #[case::response_trigger_no_history(
        "{% response 'raw', 'req', 'b64::JC5kYXRh::46b', 'no-history', 60 %}",
        "{{ response('req', trigger='no_history') }}"
    )]
    #[case::response_trigger_expire(
        "{% response 'raw', 'req', 'b64::JC5kYXRh::46b', 'when-expired', 60 %}",
        "{{ response('req', trigger='60s') }}"
    )]
    #[case::response_trigger_always(
        "{% response 'raw', 'req', 'b64::JC5kYXRh::46b', 'always', 60 %}",
        "{{ response('req', trigger='always') }}"
    )]
    fn test_parse_template(#[case] input: &str, #[case] expected: Template) {
        let actual = parse_template(input.to_owned());
        assert_eq!(actual, expected);
    }
}
