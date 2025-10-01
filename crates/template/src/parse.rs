//! Template parsing

use crate::{
    Template, TemplateChunk,
    error::TemplateParseError,
    expression::{Expression, FunctionCall, Identifier, Literal},
};
use indexmap::IndexMap;
use std::{convert, str::FromStr, sync::Arc};
use winnow::{
    ModalParser, ModalResult, Parser,
    ascii::{dec_int, escaped, float, multispace0},
    combinator::{
        alt, cut_err, delimited, eof, fail, not, opt, peek, preceded, repeat,
        repeat_till, separated, separated_pair, terminated,
    },
    error::{ContextError, StrContext, StrContextValue},
    stream::{Accumulate, AsChar},
    token::{any, one_of, take_till, take_while},
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
    pub fn from_field(field: impl Into<Identifier>) -> Self {
        Self {
            chunks: vec![TemplateChunk::Expression(Expression::Field(
                field.into(),
            ))],
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
    /// Is the character allowed in an identifier?
    fn is_char_allowed(c: char) -> bool {
        Self::is_char_allowed_first(c) || c.is_numeric() || c == '-'
    }

    /// Is the character allowed as the first character in an identifier?
    fn is_char_allowed_first(c: char) -> bool {
        c.is_alphabetic() || c == '_'
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
                .enumerate()
                .map(|(i, c)| {
                    if i == 0 && Self::is_char_allowed_first(c)
                        || (i > 0 && Self::is_char_allowed(c))
                    {
                        c
                    } else {
                        '_'
                    }
                })
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
/// - <https://docs.rs/winnow/latest/winnow/_topic/performance/index.html>
fn all_chunks(input: &mut &str) -> ModalResult<Vec<TemplateChunk>> {
    repeat_till(
        0..,
        alt((
            expression_chunk.map(TemplateChunk::Expression),
            raw.map(TemplateChunk::Raw),
        ))
        .context(ctx_label("template chunk")),
        eof,
    )
    .map(|(chunks, _)| chunks)
    .context(ctx_label("template"))
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
    .context(ctx_label("raw text"))
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
    preceded(
        EXPRESSION_OPEN,
        // Any error inside a template key is fatal, including an unclosed key
        cut_err(terminated(expression, EXPRESSION_CLOSE)),
    )
    .context(ctx_label("expression"))
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
    ws(terminated(
        alt((
            literal.map(Expression::Literal),
            array.map(Expression::Array),
            object.map(Expression::Object),
            // Look for a fn call before a field to reduce backtracking. A fn
            // call will always parse as a field, but not vice versa
            call.map(Expression::Call),
            identifier.map(Expression::Field),
            // If all cases fail, the error from the last case is used. But we
            // want to report an error of "invalid expression" instead
            fail.context(ctx_expected("literal"))
                .context(ctx_expected("array"))
                .context(ctx_expected("function call"))
                .context(ctx_expected("field")),
        )),
        boundary,
    ))
    .context(ctx_label("expression"))
    .parse_next(input)
}

/// Parse a literal: null, bool, int, float, string
fn literal(input: &mut &str) -> ModalResult<Literal> {
    alt((
        NULL.map(|_| Literal::Null),
        FALSE.map(|_| Literal::Boolean(false)),
        TRUE.map(|_| Literal::Boolean(true)),
        // If we see a number with a . or e/E (for scientific notation), it's a
        // float. Otherwise it's an int. We need to do this peek check to
        // prevent the int parser from eating the first half of a float and
        // leaving us in an unrecoverable state. We can't put the float parser
        // first because it would consume all ints.
        preceded(
            peek((
                opt('-'),
                take_while(1.., |c: char| c.is_ascii_digit()),
                one_of(['.', 'e', 'E']),
            )),
            float.map(Literal::Float).context(ctx_label("float")),
        ),
        dec_int.map(Literal::Integer).context(ctx_label("int")),
        string_literal,
        byte_literal,
    ))
    .parse_next(input)
}

/// Parse a string literal: '...' or "..."
fn string_literal(input: &mut &str) -> ModalResult<Literal> {
    // " and ' can only mean string literal, so an error after the open is fatal
    alt((
        quoted_literal('\'', convert::identity, convert::identity, fail),
        quoted_literal('"', convert::identity, convert::identity, fail),
    ))
    .map(Literal::String)
    .context(ctx_label("string literal"))
    .parse_next(input)
}

/// Parse a byte literal: b'...' or b"..."
fn byte_literal(input: &mut &str) -> ModalResult<Literal> {
    /// Parse an escaped byte code: \x00
    fn byte_code(input: &mut &str) -> ModalResult<u8> {
        preceded(
            "x",
            // Once we've seen \x, we expect exactly two hex digits
            cut_err(
                take_while(2, AsChar::is_hex_digit)
                    // We know we have two hex digits, so the parse can't fail
                    .map(|s| u8::from_str_radix(s, 16).unwrap())
                    .context(ctx_label("byte code"))
                    .context(StrContext::Expected(
                        StrContextValue::Description(
                            "two hex digits [0-9a-fA-F]",
                        ),
                    )),
            ),
        )
        .parse_next(input)
    }

    preceded(
        "b",
        alt((
            quoted_literal('\'', str::as_bytes, |c| c as u8, byte_code),
            quoted_literal('"', str::as_bytes, |c| c as u8, byte_code),
        )),
    )
    .map(|bytes: Vec<u8>| Literal::Bytes(bytes.into()))
    .context(ctx_label("byte literal"))
    .parse_next(input)
}

/// Parse an array: [expr, ...]
fn array(input: &mut &str) -> ModalResult<Vec<Expression>> {
    (delimited_list('[', expression, ']'))
        .context(ctx_label("array"))
        .parse_next(input)
}

/// Parse an object: {"key": expr, ...}
fn object(input: &mut &str) -> ModalResult<Vec<(Expression, Expression)>> {
    (delimited_list('{', separated_pair(expression, ws(":"), expression), '}'))
        .context(ctx_label("object"))
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
                .context(ctx_label("keyword argument")),
            expression
                .map(Argument::Position)
                .context(ctx_label("positional argument")),
        ))
        .parse_next(input)
    }

    // Parse arguments as a mixed list of positional and keyword args. We'll
    // then unpack the list and make sure all positional arguments are first.
    // This makes it a bit easier to provide useful errors when kwargs appear
    // before positional args.
    let (name, arguments): (Identifier, Vec<Argument>) = (
        identifier.context(ctx_label("function name")),
        delimited_list('(', argument, ')'),
    )
        .context(ctx_label("function call"))
        .parse_next(input)?;

    // Unpack the args and look for two error cases:
    // - Positional arg after kwarg
    // - Repeated kwarg
    let mut position: Vec<Expression> = Vec::new();
    let mut keyword: IndexMap<Identifier, Expression> = IndexMap::new();
    for argument in arguments {
        match argument {
            Argument::Position(expression) => {
                if !keyword.is_empty() {
                    return cut_err(fail)
                        .context(ctx_label(
                            "positional argument after keyword argument",
                        ))
                        .context(ctx_expected(
                            "keyword arguments to be after \
                            positional arguments",
                        ))
                        .parse_next(input);
                }
                position.push(expression);
            }
            Argument::Keyword(name, expression) => {
                if keyword.insert(name, expression).is_some() {
                    return cut_err(fail)
                        .context(ctx_label("duplicate keyword argument"))
                        .context(ctx_expected("keyword arguments to be unique"))
                        .parse_next(input);
                }
            }
        }
    }

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
        cut_err(call.context(ctx_expected("function call"))),
    )
    .context(ctx_label("pipe"))
    .parse_next(input)
}

/// Create a parser for a comma-separated list with bounding delimiters.
/// Supports an optional trailing comma and whitespace around each element. The
/// open delimiter must be unambiguous, such that any error after the open is
/// fatal.
fn delimited_list<'a, O, Acc, F>(
    open: char,
    parser: F,
    close: char,
) -> impl ModalParser<&'a str, Acc, ContextError>
where
    F: ModalParser<&'a str, O, ContextError>,
    Acc: Accumulate<O>,
{
    preceded(
        open,
        // Delimiters are unambiguous, so once we see the open any error is
        // fatal
        cut_err(terminated(
            ws(terminated(
                separated(0.., parser, ws(",")), // Comma-separated elements
                opt(ws(",")),                    // Optional trailing comma
            )),
            close.context(StrContext::Expected(StrContextValue::CharLiteral(
                close,
            ))),
        )),
    )
}

/// Create a parser for some contents bounded by a symmetrical delimiter, e.g.
/// a string or byte literal. Supports escape sequences using \.
///
/// ## Params
///
/// - `quote_char` - Delimiter character (`'` or `"`)
/// - `map_contents` - Function to map unescaped contents to the output type
/// - `map_escape` - Function to map escaped characters to the output type
/// - `escape` - Parser for escaped characters within the literal
fn quoted_literal<'a, Output, MapOutput, EscapeOutput>(
    quote_char: char,
    map_contents: impl (Fn(&'a str) -> MapOutput) + Copy,
    map_escape: impl (Fn(char) -> EscapeOutput) + Copy,
    escape: impl ModalParser<&'a str, EscapeOutput, ContextError>,
) -> impl ModalParser<&'a str, Output, ContextError>
where
    Output: Accumulate<MapOutput> + Accumulate<EscapeOutput>,
{
    // The opening quote is unambiguous, so once we've seen it, errors are fatal
    preceded(
        quote_char,
        cut_err(terminated(
            escaped(
                // escaped() requires this to take 1+ chars
                take_till(1.., move |c| c == quote_char || c == '\\')
                    .map(map_contents),
                '\\',
                alt((
                    alt((
                        "\\".value('\\'),
                        "n".value('\n'),
                        "r".value('\r'),
                        "t".value('\t'),
                        quote_char,
                    ))
                    .map(map_escape),
                    escape,
                )),
            ),
            cut_err(quote_char.context(StrContext::Expected(
                StrContextValue::CharLiteral(quote_char),
            ))),
        )),
    )
}

/// Wrap a parser to allow whitespace on either side of it
fn ws<'a, O, F>(parser: F) -> impl ModalParser<&'a str, O, ContextError>
where
    F: ModalParser<&'a str, O, ContextError>,
{
    delimited(multispace0, parser, multispace0)
}

/// Detect the end of a token without consuming any input. This parser is used
/// after parsing an expression to ensure we got the entire token. For example,
/// it prevents parsing `1user` as a number with lingering input.
fn boundary(input: &mut &str) -> ModalResult<()> {
    // A token boundary is the same set of characters that cannot be included
    // in an identifier, as an identifier is a superset of what's allowed in
    // number literals.
    if input.is_empty()
        || !Identifier::is_char_allowed(input.chars().next().unwrap())
    {
        Ok(())
    } else {
        cut_err(fail)
            .context(ctx_expected("end of token"))
            .parse_next(input)
    }
}

/// Parse a field name/chain ID/env variable etc, inside a key. See [Identifier]
/// for the definition of allowed syntax.
fn identifier(input: &mut &str) -> ModalResult<Identifier> {
    (
        // The first char must be a letter, so if we see that we're
        // unambiguously in an identifier. Any error after is fatal.
        take_while(1, Identifier::is_char_allowed_first),
        cut_err(take_while(0.., Identifier::is_char_allowed)),
    )
        .take()
        .map(|id: &str| Identifier(id.to_owned()))
        .context(ctx_label("identifier"))
        .parse_next(input)
}

/// Create a [StrContext::Label]
fn ctx_label(label: &'static str) -> StrContext {
    StrContext::Label(label)
}

/// Create a [StrContext::Expected]
fn ctx_expected(expected: &'static str) -> StrContext {
    StrContext::Expected(StrContextValue::Description(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionCall, Literal};
    use proptest::proptest;
    use rstest::rstest;
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
    }

    /// Test parsing with extra whitespace. These strings don't round trip to
    /// the same thing so they're separate from test_parse_display
    #[rstest]
    #[case::no_whitespace("{{ field1 }}", [field_chunk("field1")])]
    #[case::bonus_whitespace("{{   field1   }}", [field_chunk("field1")])]
    #[case::object(
        "{{{'a': 1}}}", [object([(literal("a"), literal(1))]).into()],
    )]
    #[case::object_double(
        // This looks like a second template key open but it's not!
        "{{{{'a':1}:2}}}",
        [object([(object([(literal("a"), literal(1))]), literal(2))]).into()],
    )]
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
    #[case::trailing_dot("{{bogus.}}", "invalid expression")]
    // the first { is escaped, 2nd and 3rd make the expression, 4th opens an
    // object that never gets closed
    #[case::bonus_braces(r"\\{{{{field}}", "invalid object\nexpected `}`")]
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
    // ===== Primitive literals =====
    #[case::literal_null("null", literal(Literal::Null), None)]
    #[case::literal_bool_false("false", literal(false), None)]
    #[case::literal_bool_true("true", literal(true), None)]
    #[case::literal_int_negative("-10", literal(-10),None)]
    #[case::literal_int_positive("17", literal(17), None)]
    #[case::literal_int_min("-9223372036854775808", literal(i64::MIN), None)]
    #[case::literal_int_max("9223372036854775807", literal(i64::MAX), None)]
    #[case::literal_float_negative("-3.5", literal(-3.5), None)]
    #[case::literal_float_positive("3.5", literal(3.5), None)]
    #[case::literal_float_scientific("3.5e3", literal(3500.0), Some("3500.0"))]
    // ===== String literals =====
    #[case::literal_string_single("'hello'", literal("hello"), None)]
    #[case::literal_string_single_empty("''", literal(""), None)]
    #[case::literal_string_single_escape(
        r"'hello \'\n\t\r\\'",
        literal("hello '\n\t\r\\"),
        None
    )]
    // Double quote strings display back to single quotes
    #[case::literal_string_double_empty("\"\"", literal(""), Some("''"))]
    #[case::literal_string_double_escape(
        r#""hello \"""#,
        literal("hello \""),
        Some("'hello \"'")
    )]
    #[case::literal_string_double(
        "\"hello\"",
        literal("hello"),
        Some("'hello'")
    )]
    // ===== Bytes literals =====
    #[case::literal_bytes_single(
        r"b'hello\xc3\x28'",
        literal(b"hello\xc3\x28"),
        Some(r"b'hello\xc3('")
    )]
    #[case::literal_bytes_single_escape(
        r"b'hello \'\n\t\r\\'",
        literal(b"hello '\n\t\r\\"),
        // Non-graphic ASCII characters are reprinted as their byte codes
        Some(r"b'hello \'\x0a\x09\x0d\\'")
    )]
    // Double quote byte strings display back to single quotes
    #[case::literal_bytes_double(
        r#"b"hello\xc3\x28""#,
        literal(b"hello\xc3\x28"),
        Some(r"b'hello\xc3('")
    )]
    #[case::literal_bytes_double_escape(
        r#"b"hello \"\n\t\r\\""#,
        literal(b"hello \"\n\t\r\\"),
        Some(r#"b'hello "\x0a\x09\x0d\\'"#)
    )]
    // ===== Array literals =====
    #[case::array(
        "[1, 'hi', field]",
        array([literal(1), literal("hi"), field("field")]),
        None,
    )]
    #[case::array_trailing_comma("[1,]", array([literal(1)]), Some("[1]"))]
    // ===== Object literals =====
    #[case::object(
        "{1: 1, 'a': 'hi', field: field}",
        object([
            (literal(1), literal(1)),
            (literal("a"), literal("hi")),
            (field("field"), field("field")),
        ]),
        None,
    )]
    #[case::object_trailing_comma(
        "{'a':1,}", object([(literal("a"), literal(1))]), Some("{'a': 1}"),
    )]
    #[case::object_whitespace(
        " { 'a' : 1 } ", object([(literal("a"), literal(1))]), Some("{'a': 1}"),
    )]
    // ===== Fields =====
    #[case::field("field1", field("field1"), None)]
    // ===== Function calls =====
    #[case::function(
        "f(1, 'hi', a=field)",
        call("f", [literal(1), literal("hi")], [("a", field("field"))]),
        None
    )]
    #[case::function_nested(
        "f(g(h()))",
        call("f", [call("g", [call("h", [], [])], [])], []),
        None
    )]
    #[case::function_trailing_comma(
        "f(1,)", call("f", [literal(1)], []), Some("f(1)")
    )]
    // ===== Pipes =====
    #[case::pipe(
        "f(1, a=2) | g(3, a=4)",
        pipe(
            call("f", [literal(1)], [("a", literal(2))]),
            call("g", [literal(3)], [("a", literal(4))]),
        ),
        None
    )]
    // Pipes are left-associative
    #[case::pipe_chain(
        "1 | f() | g()",
        pipe(pipe(literal(1),  call("f", [], [])), call("g", [], [])),None
    )]
    #[case::pipe_nested(
        "f(1 | g())",
        call("f", [pipe(literal(1), call("g", [], []))], []),None
    )]
    fn test_parse_display_expression(
        #[case] input: &'static str,
        #[case] expected: Expression,
        #[case] expected_display: Option<&'static str>,
    ) {
        let parsed: Expression = expression
            .parse(input)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed, expected, "incorrect parsed expression");
        let stringified = parsed.to_string();
        let expected_str = expected_display.unwrap_or(input);
        assert_eq!(
            stringified, expected_str,
            "incorrect stringified expression"
        );
    }

    /// Test parsing error cases for expressions
    #[rstest]
    #[case::field_leading_number("1user", "invalid expression")]
    #[case::field_leading_dash("-user", "invalid expression")]
    #[case::function_incomplete("bogus(", "invalid function call")]
    #[case::function_dupe_kwarg(
        "f(a=1, a=2)",
        "invalid duplicate keyword argument"
    )]
    #[case::function_positional_after_kwarg(
        "f(a=1, 2)",
        "invalid positional argument after keyword argument"
    )]
    #[case::pipe_incomplete("bogus |", "expected function call")]
    #[case::pipe_to_literal("f() | 3", "expected function call")]
    // This case is common because Jinja allows this when piping to filters
    #[case::pipe_to_identifier("f() | trim", "expected function call")]
    // Make sure errors within a function arg are handled correctly
    #[case::invalid_function_arg(
        "command(\"invalid)",
        "invalid string literal"
    )]
    #[case::array_incomplete("[bogus", "invalid array")]
    #[case::string_incomplete("'bogus", "invalid string")]
    #[case::bytes_incomplete("b'bogus", "invalid byte literal")]
    #[case::bytes_invalid_code_short(r"b'\x2'", "invalid byte code")]
    #[case::bytes_invalid_code_not_hex(r"b'\x2w'", "invalid byte code")]
    fn test_parse_expression_error(
        #[case] input: &str,
        #[case] expected_error: &str,
    ) {
        assert_err!(expression.parse(input), expected_error);
    }

    /// Test that [Template::from_field] generates the correct template
    #[test]
    fn test_from_field() {
        let template = Template::from_field("field1");
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
    #[case::leading_number("1not", "_not")]
    #[case::leading_dash("-not", "_not")]
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

    /// Shorthand for creating an object literal expression
    fn object<const N: usize>(
        entries: [(Expression, Expression); N],
    ) -> Expression {
        Expression::Object(entries.into())
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
