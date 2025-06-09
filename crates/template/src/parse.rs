//! Template parsing

use crate::{
    Template, TemplateChunk,
    error::TemplateParseError,
    expression::{Expression, FunctionCall, Identifier, Literal},
};
use indexmap::IndexMap;
use std::{str::FromStr, sync::Arc};
use winnow::{
    ModalResult, Parser,
    ascii::{dec_int, float, multispace0},
    combinator::{
        alt, cut_err, delimited, eof, not, opt, peek, preceded, repeat,
        repeat_till, separated, separated_pair, terminated,
    },
    error::{ContextError, ErrMode, ParserError, StrContext},
    stream::Accumulate,
    token::{any, take_while},
};

/// Character used to escape key openings
pub(crate) const ESCAPE: &str = "_";
/// Marks the start of a template key
pub(crate) const EXPRESSION_OPEN: &str = "{{";
/// Marks the end of a template key
pub(crate) const EXPRESSION_CLOSE: &str = "}}";
pub(crate) const NULL: &str = "null";
pub(crate) const FALSE: &str = "false";
pub(crate) const TRUE: &str = "true";

impl Template {
    /// Create a template that renders a single field, equivalent to
    /// `{{ <field> }}`
    pub fn from_field(field: Identifier) -> Self {
        Self {
            chunks: vec![TemplateChunk::Expression(Expression::Field(field))],
        }
    }
}

/// Parse a template, extracting all template keys
impl FromStr for Template {
    type Err = TemplateParseError;

    fn from_str(template: &str) -> Result<Self, Self::Err> {
        let chunks = all_chunks.parse(template)?;
        Ok(Self { chunks })
    }
}

impl Identifier {
    /// Which characters are allowed in identifiers?
    fn is_char_allowed(c: char) -> bool {
        c.is_alphanumeric() || "-_".contains(c)
    }

    /// Generate an identifier from a string, replacing all invalid chars with
    /// a placeholder. Panic if the string is empty.
    pub fn escape(value: &str) -> Self {
        assert!(
            !value.is_empty(),
            "Cannot create identifier from empty string"
        );
        Self(
            value
                .chars()
                .map(|c| if Self::is_char_allowed(c) { c } else { '_' })
                .collect(),
        )
    }
}

impl FromStr for Identifier {
    type Err = TemplateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(identifier.parse(s)?)
    }
}

/// Parse a template into keys and raw text
///
/// Potential optimizations if parsing is slow:
/// - Use take_till or similar in raw string parsing
/// - https://docs.rs/winnow/latest/winnow/_topic/performance/index.html
fn all_chunks(input: &mut &str) -> ModalResult<Vec<TemplateChunk>> {
    repeat_till(
        0..,
        alt((
            expression_chunk.map(TemplateChunk::Expression),
            raw.map(TemplateChunk::Raw),
        ))
        .context(StrContext::Label("template chunk")),
        eof,
    )
    .map(|(chunks, _)| chunks)
    .context(StrContext::Label("template"))
    .parse_next(input)
}

/// Parse raw text, until we hit a key or end of input
fn raw(input: &mut &str) -> ModalResult<Arc<str>> {
    repeat(
        0..,
        alt((
            escape_sequence,
            // Match anything other than a key opening. This is inefficient
            // because it means we'll copy into the accumulating string one
            // char at a time. We could theoretically grab up to the next
            // escape seq or key here but I couldn't figure that out. Potential
            // optimization if perf is a problem
            (not(EXPRESSION_OPEN), any).take(),
        )),
    )
    .map(String::into)
    .context(StrContext::Label("raw text"))
    .parse_next(input)
}

/// Match an escape sequence `{_{`, `{__}`, etc. The trailing curly brace will
/// **not** be consumed.
fn escape_sequence<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
    terminated(
        // Parse {_+
        ("{", repeat::<_, _, (), _, _>(1.., ESCAPE))
            .take()
            // Drop the final underscore
            .map(|s: &str| &s[..s.len() - 1]),
        // Throw away the final _, don't consume the trailing {
        peek("{"),
    )
    .parse_next(input)
}

/// Parse a template expression with its bounding `{{ }}`
fn expression_chunk(input: &mut &str) -> ModalResult<Expression> {
    delimited(
        EXPRESSION_OPEN,
        // Any error inside a template key is fatal, including an unclosed key
        cut_err(expression),
        EXPRESSION_CLOSE,
    )
    .context(StrContext::Label("expression"))
    .parse_next(input)
}

/// Parse the contents of an expression (inside the `{{ }}` or nested within
/// another expression)
fn expression(input: &mut &str) -> ModalResult<Expression> {
    // The first expression can be anything but a pipe. We have to exclude pipes
    // here to avoid infinite recursion.
    let first_expression = primary_expression.parse_next(input)?;
    // Now that we have an expression, we can check for pipes. Pipes are
    // left-associative: `expr | f() | g()` = `g(f(expr))`
    // It would be nice to use repeat().fold() here to build the expression
    // directly without having to allocate to a vec, but fold() takes the
    // initializer as FnMut which would require us to clone the primary
    // expression.
    // https://github.com/winnow-rs/winnow/issues/513
    let pipes: Vec<FunctionCall> =
        repeat(0.., ws(pipe_call)).parse_next(input)?;
    Ok(pipes
        .into_iter()
        .fold(first_expression, |acc, call| Expression::Pipe {
            expression: Box::new(acc),
            call,
        }))
}

/// Parse an initial inner expression. This can be anything *but* a pipe. We
/// have to exclude pipes here because they are left-recursive, which triggers
/// an infinite loop in parsing.
fn primary_expression(input: &mut &str) -> ModalResult<Expression> {
    ws(alt((
        literal.map(Expression::Literal),
        array.map(Expression::Array),
        // Look for a fn call before a field to reduce backtracking. A fn
        // call will always parse as a field, but not vice versa
        call.map(Expression::Call),
        identifier.map(Expression::Field),
    )))
    .context(StrContext::Label("expression"))
    .parse_next(input)
}

/// Parse a literal: null, bool, int, float, string
fn literal(input: &mut &str) -> ModalResult<Literal> {
    alt((
        NULL.map(|_| Literal::Null),
        FALSE.map(|_| Literal::Bool(false)),
        TRUE.map(|_| Literal::Bool(true)),
        // int must come before float because all ints are valid floats too
        dec_int.map(Literal::Int).context(StrContext::Label("int")),
        float
            .map(Literal::Float)
            .context(StrContext::Label("float")),
        string_literal,
    ))
    .parse_next(input)
}

/// Parse a string literal: '...' or "..."
fn string_literal(input: &mut &str) -> ModalResult<Literal> {
    // " and ' can only mean string literal, so an error after the open is fatal
    alt((
        delimited("\"", cut_err(take_while(0.., |c| c != '"')), "\""),
        delimited("'", cut_err(take_while(0.., |c| c != '\'')), "'"),
    ))
    .map(|s: &str| Literal::String(s.to_owned()))
    .context(StrContext::Label("string literal"))
    .parse_next(input)
}

/// Parse an array: [expr, ...]
fn array(input: &mut &str) -> ModalResult<Vec<Expression>> {
    (delimited(
        "[",
        // [] can only mean arrays, so an error after [ is fatal
        cut_err(ws(comma_separated(expression))),
        "]",
    ))
    .context(StrContext::Label("array"))
    .parse_next(input)
}

/// Parse a function call: `f(...)`
fn call(input: &mut &str) -> ModalResult<FunctionCall> {
    enum Argument {
        Position(Expression),
        Keyword(Identifier, Expression),
    }

    /// Parse a single positional or keyword argument
    fn argument(input: &mut &str) -> ModalResult<Argument> {
        alt((
            // Parse kwarg first because it's more specific
            separated_pair(identifier, ws('='), expression)
                .map(|(name, expression)| Argument::Keyword(name, expression))
                .context(StrContext::Label("keyword argument")),
            expression
                .map(Argument::Position)
                .context(StrContext::Label("positional argument")),
        ))
        .parse_next(input)
    }

    // Parse arguments as a mixed list of positional and keyword args. We'll
    // then unpack the list and make sure all positional arguments are first.
    // This makes it a bit easier to provide useful errors when kwargs appear
    // before positional args.
    let (name, arguments): (Identifier, Vec<Argument>) = (
        identifier,
        delimited(
            "(",
            // Parens are only used for function calls, so any error is fatal
            cut_err(ws(comma_separated(argument))),
            ")",
        )
        .context(StrContext::Label("function arguments")),
    )
        .context(StrContext::Label("function call"))
        .parse_next(input)?;
    // Unpack the args and look for two error cases:
    // - Positional arg after kwarg
    // - Repeated kwarg
    let (position, keyword) = arguments.into_iter().try_fold(
        (Vec::new(), IndexMap::new()),
        |(mut arguments, mut kwargs), argument| {
            match argument {
                Argument::Position(expression) => {
                    if !kwargs.is_empty() {
                        return Err(ErrMode::assert(
                            input,
                            "position after keyword",
                        ));
                    }
                    arguments.push(expression);
                }
                Argument::Keyword(name, expression) => {
                    if kwargs.insert(name, expression).is_some() {
                        return Err(ErrMode::assert(input, "repeat kwarg"));
                    }
                }
            }
            Ok((arguments, kwargs))
        },
    )?;
    Ok(FunctionCall {
        function: name,
        position,
        keyword,
    })
}

/// Parse the right side of a pipe expression: `| call()`
/// We can't parse a whole pipe expression at once because it creates a
/// left-recursive grammar and the parser enters an infinite loop. The parent
/// parser is responsible for parsing a valid expression before the pipe.
fn pipe_call(input: &mut &str) -> ModalResult<FunctionCall> {
    preceded(
        ws("|"),
        // Once we've hit a |, the only possible option is a pipe so an error
        // on the right side is fatal
        cut_err(call),
    )
    .context(StrContext::Label("pipe"))
    .parse_next(input)
}

/// Repeat a parser with commas separating each element. Supports an optional
/// trailing comma
fn comma_separated<'a, O, Acc, F>(
    parser: F,
) -> impl Parser<&'a str, Acc, ContextError>
where
    F: Parser<&'a str, O, ContextError>,
    Acc: Accumulate<O>,
{
    terminated(separated(0.., parser, ws(",")), opt(ws(",")))
}

/// Wrap a parser to allow whitespace on either side of it
fn ws<'a, O, F>(parser: F) -> impl Parser<&'a str, O, ContextError>
where
    F: Parser<&'a str, O, ContextError>,
{
    delimited(multispace0, parser, multispace0)
}

/// Parse a field name/chain ID/env variable etc, inside a key. See [Identifier]
/// for the definition of allowed syntax.
fn identifier(input: &mut &str) -> ModalResult<Identifier> {
    take_while(1.., Identifier::is_char_allowed)
        .map(|id: &str| Identifier(id.to_owned()))
        .context(StrContext::Label("identifier"))
        .parse_next(input)
}

#[cfg(test)]
mod tests {
    use crate::{FunctionCall, Literal};

    use super::*;
    use proptest::proptest;
    use rstest::rstest;
    use serde_test::{Token, assert_tokens};
    use slumber_util::{assert_err, assert_matches};
    use std::borrow::Cow;

    /// Test round tripping between raw strings and templates. Parse, display,
    /// make sure we get the same thing back. Also check if stringification
    /// allocated, to make sure optimizations work as intended.
    ///
    /// The round trip doesn't always give the same thing back as whitespace
    /// within expressions is variable. This test uses standard whitespace in
    /// the expressions to enable the round tripping. A separate test case
    /// one-way string->template parsing.
    #[rstest]
    #[case::empty("", [])]
    #[case::whitespace("   ", [raw("   ")])]
    #[case::raw("raw", [raw("raw")])]
    #[case::unopened_key("unopened}}", [raw("unopened}}")])]
    #[case::field("{{ field1 }}", [field_chunk("field1")])]
    // Other expression types are tested as singular expressions
    #[case::utf8(
        "intro\n{{ user_id }} 💚💙💜 {{ user_name }}\noutro\r\nmore outro",
        [
            raw("intro\n"),
            field_chunk("user_id"),
            raw(" 💚💙💜 "),
            field_chunk("user_name"),
            raw("\noutro\r\nmore outro"),
        ],
    )]
    #[case::binary(r"\xc3\x28", [raw(r"\xc3\x28")])]
    #[case::escape_incomplete_key("{_{hello {_{_{", [raw("{{hello {{{")])]
    #[case::escape_key(
        // You should be able to put any number of underscores within a key,
        // and get n-1
        "{_{ {__{{ user_id }} {___{{ user_id }} {___{__{{ user_id }}",
        [
            raw("{{ {_"),
            field_chunk("user_id"),
            raw(" {__"),
            field_chunk("user_id"),
            raw(" {__{_"),
            field_chunk("user_id"),
        ],
    )]
    // `{_` should be treated literally when not followed by another {
    #[case::literal_underscores("{_a {_ _{", [raw("{_a {_ _{")])]
    fn test_parse_display_template(
        #[case] input: &'static str,
        #[case] expected: impl Into<Template>,
    ) {
        let expected = expected.into();
        let parsed: Template = input.parse().expect("Parsing failed");
        assert_eq!(parsed, expected, "incorrect parsed template");
        let stringified = parsed.display();
        assert_eq!(stringified, input, "incorrect stringified template");

        // Display should avoid allocation if the template is empty or a single
        // raw chunk whose text matches the original source (i.e. it hasn't had
        // any escape characters removed)
        let display_should_allocate = match expected.chunks.as_slice() {
            [] => false,
            [TemplateChunk::Raw(text)] => &**text != input,
            _ => true,
        };
        if display_should_allocate {
            assert_matches!(stringified, Cow::Owned(_));
        } else {
            assert_matches!(stringified, Cow::Borrowed(_));
        }

        // Make sure serialization/deserialization impls work too
        assert_tokens(&expected, &[Token::Str(input)]);
    }

    /// Test parsing with extra whitespace. These strings don't round trip to
    /// the same thing so they're separate from test_parse_display
    #[rstest]
    #[case::no_whitespace("{{field1}}", [field_chunk("field1")])]
    #[case::bonus_whitespace("{{   field1   }}", [field_chunk("field1")])]
    fn test_parse_template(
        #[case] input: &'static str,
        #[case] expected: impl Into<Template>,
    ) {
        let parsed: Template = input.parse().expect("Parsing failed");
        assert_eq!(parsed, expected.into(), "incorrect parsed template");
    }

    /// Test parsing error cases
    #[rstest]
    #[case::unclosed_expression("{{", "invalid expression")]
    #[case::empty_expression("{{}}", "invalid expression")]
    #[case::invalid_expression("{{.}}", "invalid expression")]
    // the first { is escaped, 2nd and 3rd make the expression, 4th is a problem
    #[case::bonus_braces(r"\\{{{{field}}", "invalid expression")]
    fn test_parse_template_error(
        #[case] template: &str,
        #[case] expected_error: &str,
    ) {
        assert_err!(template.parse::<Template>(), expected_error);
    }

    /// Test round tripping between raw strings and expressions. Parse, display,
    /// make sure we get the same thing back. It's easier to test individual
    /// expressions outside the context of a template.
    ///
    /// The round trip doesn't always give the same thing back as whitespace
    /// within expressions is variable. This test uses standard whitespace in
    /// the expressions to enable the round tripping. A separate test case
    /// one-way string->template parsing.
    #[rstest]
    #[case::literal_null("null", literal(Literal::Null))]
    #[case::literal_bool_false("false", literal(false))]
    #[case::literal_bool_true("true", literal(true))]
    #[case::literal_int_negative("-10", literal(-10))]
    #[case::literal_int_positive("17", literal(17))]
    #[case::literal_int_min("-9223372036854775808", literal(i64::MIN))]
    #[case::literal_int_max("9223372036854775807", literal(i64::MAX))]
    #[case::literal_float_negative("-3.5", literal(-3.5))]
    #[case::literal_float_positive("3.5", literal(3.5))]
    #[case::literal_float_scientific("3.5e3", literal(3500.0))]
    #[case::literal_string_double("\"hello\"", literal("hello"))]
    #[case::literal_string_double_escape(r#""hello \"""#, literal("hello \""))]
    #[case::field("field1", field("field1"))]
    #[case::array(
        "[1, \"hi\", field]", array([literal(1), literal("hi"), field("field")]),
    )]
    #[case::function(
        "f(1, \"hi\", a=field)",
        call("f", [literal(1), literal("hi")], [("a", field("field"))]),
    )]
    #[case::function_nested(
        "f(g(h()))",
        call("f", [call("g", [call("h", [], [])], [])], []),
    )]
    #[case::pipe(
        "f(1, a=2) | g(3, a=4)",
        pipe(
            call("f", [literal(1)], [("a", literal(2))]),
            call("g", [literal(3)], [("a", literal(4))]),
        ),
    )]
    // Pipes are left-associative
    #[case::pipe_chain(
        "1 | f() | g()",
        pipe(pipe(literal(1),  call("f", [], [])), call("g", [], [])),
    )]
    #[case::pipe_nested(
        "f(1 | g())",
        call("f", [pipe(literal(1), call("g", [], []))], []),
    )]
    fn test_parse_display_expression(
        #[case] input: &'static str,
        #[case] expected: Expression,
    ) {
        let parsed: Expression = expression
            .parse(input)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed, expected, "incorrect parsed expression");
        let stringified = parsed.to_string();
        assert_eq!(stringified, input, "incorrect stringified expression");
    }

    /// Test parsing expressions that don't round trip
    #[rstest]
    #[case::literal_string_single("'hello'", literal("hello"))]
    #[case::literal_string_single_escape(r"'hello \'", literal("hello '"))]
    #[case::array_trailing_comma("[1,]", array([literal(1)]))]
    #[case::function_trailing_comma("f(1,)", call("f", [literal(1)], []))]
    fn test_parse_expression(
        #[case] input: &'static str,
        #[case] expected: Expression,
    ) {
        let parsed: Expression = expression
            .parse(input)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed, expected, "incorrect parsed expression");
    }

    /// Test parsing error cases for expressions
    #[rstest]
    #[case::function_incomplete("bogus(", "invalid key")]
    #[case::function_dupe_kwarg(
        "f(a=1, a=2)",
        "duplicate keyword argument `a`"
    )]
    #[case::function_positional_after_kwarg(
        "f(a=1, 2)",
        "positional argument after keyword argument"
    )]
    #[case::pipe_incomplete("bogus |", "pipe missing right-hand side")]
    #[case::pipe_to_literal(
        "f() | 3",
        "pipe right-hand side must be function call"
    )]
    // This case is common because Jinja allows this when piping to filters
    #[case::pipe_to_identifier(
        "f() | trim",
        "pipe right-hand side must be function call"
    )]
    #[case::invalid_function_arg("command(\"invalid)", "TODO")]
    #[case::property_incomplete("bogus.", "invalid expression")]
    #[case::array_incomplete("[bogus", "unclosed array")]
    #[case::string_incomplete("\"bogus", "unclosed string")]
    fn test_parse_expression_error(
        #[case] input: &str,
        #[case] expected_error: &str,
    ) {
        assert_err!(expression.parse(input), expected_error);
    }

    /// Test that [Template::from_field] generates the correct template
    #[test]
    fn test_from_field() {
        let template = Template::from_field("field1".into());
        assert_eq!(&template.chunks, &[field("field1").into()]);
    }

    /// Test [Template::raw]. This should parse+stringify back to the same thing
    #[rstest]
    #[case::empty("", [])]
    #[case::key("{{hello}}", [raw("{{hello}}")])]
    #[case::backslash(r"\{{hello}}", [raw(r"\{{hello}}")])]
    fn test_raw(#[case] template: &str, #[case] expected: impl Into<Template>) {
        let escaped = Template::raw(template.into());
        assert_eq!(escaped, expected.into());
    }

    #[rstest]
    #[case::valid("valid-identifier_yeah", "valid-identifier_yeah")]
    #[case::invalid("not valid!", "not_valid_")]
    fn test_escape_identifier(#[case] input: &str, #[case] expected: &str) {
        let parsed = Identifier::escape(input);
        assert_eq!(parsed.as_str(), expected);
    }

    /// Escaping an empty identifier panics
    #[test]
    #[should_panic(expected = "Cannot create identifier from empty string")]
    fn test_escape_identifier_empty() {
        Identifier::escape("");
    }

    proptest! {
        /// Proptest that generates a valid template, stringifies it, then
        /// parses it back. Should always give back the same template
        #[test]
        fn test_round_trip_prop(template: Template) {
            let s = template.display();
            let parsed = s.parse::<Template>()
                .expect("Error parsing stringified template");
            assert_eq!(parsed, template);
        }
    }

    /// Shorthand for creating a new raw chunk
    fn raw(value: &str) -> TemplateChunk {
        TemplateChunk::Raw(value.to_owned().into())
    }

    /// Shorthand for creating a field expression chunk
    fn field_chunk(f: &'static str) -> TemplateChunk {
        field(f).into()
    }

    /// Shorthand for creating a literal expression
    fn literal(l: impl Into<Literal>) -> Expression {
        Expression::Literal(l.into())
    }

    /// Shorthand for creating a field expression
    fn field(f: &'static str) -> Expression {
        Expression::Field(f.into())
    }

    /// Shorthand for creating an array literal expression
    fn array<const N: usize>(expressions: [Expression; N]) -> Expression {
        Expression::Array(expressions.into())
    }

    /// Shorthand for creating a function call expression
    fn call<const P: usize, const KW: usize>(
        name: &'static str,
        position: [Expression; P],
        keyword: [(&'static str, Expression); KW],
    ) -> Expression {
        Expression::Call(FunctionCall {
            function: name.into(),
            position: position.into(),
            keyword: keyword.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        })
    }

    /// Shorthand for creating a pipe expression
    fn pipe(lhs: Expression, rhs: Expression) -> Expression {
        // `call` returns an expression which is convenient most of the time.
        // Unwrapping it is easier than adding a second layer of abstraction
        let Expression::Call(call) = rhs else {
            panic!("Right-hand side of a pipe must be a call expression")
        };
        Expression::Pipe {
            expression: Box::new(lhs),
            call,
        }
    }
}
