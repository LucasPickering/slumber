use crate::{
    Arguments, Context, Identifier, LazyValue, RenderError, Template, Value,
    value::StreamSource,
};
use bytes::{Bytes, BytesMut};
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use indexmap::indexmap;
use rstest::rstest;
use slumber_util::{assert_err, test_data_dir};
use tokio::fs::File;
use tokio_util::io::ReaderStream;

/// Test simple expression rendering
#[rstest]
#[case::object(
    "{{ {'a': 1, 1: 2, ['a',1]: ['b',2]} }}",
    vec![
        ("a", Value::from(1)),
        ("1", 2.into()),
        // Note the whitespace in the key: it was parsed and restringified
        ("['a', 1]", vec![Value::from("b"), 2.into()].into()),
    ].into(),
)]
#[case::object_dupe_key(
    // Latest entry takes precedence
    "{{ {'Mike': 1, name: 2, 10: 3, '10': 4} }}",
    vec![("Mike", 2), ("10", 4)].into(),
)]
#[tokio::test]
async fn test_expression(#[case] template: Template, #[case] expected: Value) {
    assert_eq!(
        template
            .render(&TestContext::default())
            .await
            .try_collect_value()
            .await
            .unwrap(),
        expected
    );
}

/// Render to a value. Templates with a single dynamic chunk are allowed to
/// produce non-string values. This is specifically testing the behavior
/// of [Template::render_value], rather than expression evaluation.
#[rstest]
#[case::unpack("{{ array }}", vec!["a", "b", "c"].into())]
#[case::string("my name is {{ name }}", "my name is Mike".into())]
#[case::bytes(
    "my name is {{ invalid_utf8 }}",
    Value::Bytes(b"my name is \xc3\x28".as_slice().into(),
))]
// Stream gets resolved to bytes, then converted to a string
#[case::stream("{{ file('data.json') }}", "{ \"a\": 1, \"b\": 2 }".into())]
#[tokio::test]
async fn test_render_value(
    #[case] template: Template,
    #[case] expected: Value,
) {
    assert_eq!(
        template
            .render(&TestContext::default())
            .await
            .try_collect_value()
            .await
            .unwrap(),
        expected
    );
}

/// Render to a byte stream
#[rstest]
#[case::stream(
    "{{ file('data.json') }}",
    vec![b"{ \"a\": 1, \"b\": 2 }" as &[u8]],
)]
#[case::text(
    // Multiple chunks are chained together
    "text: {{ file('data.json') }}",
    vec![b"text: " as &[u8], b"{ \"a\": 1, \"b\": 2 }"],
)]
#[case::binary(
    "{{ invalid_utf8 }} {{ file('data.json') }}",
    vec![b"\xc3\x28" as &[u8], b" ", b"{ \"a\": 1, \"b\": 2 }"]
)]
#[tokio::test]
async fn test_render_stream(
    #[case] template: Template,
    // Assert on the individual chunks
    #[case] expected: Vec<&'static [u8]>,
) {
    let context = TestContext { can_stream: true };
    // Join into a stream, then collect the stream
    let stream = template.render(&context).await.try_into_stream().unwrap();
    let chunks = stream.try_collect::<Vec<Bytes>>().await.unwrap();
    assert_eq!(chunks, expected);
}

/// Render to a byte stream, but there's an error while rendering one of the
/// chunks
#[tokio::test]
async fn test_render_stream_chunk_error() {
    let template: Template = "{{ unknown() }}".into();
    let context = TestContext { can_stream: true };
    let result = template
        .render(&context)
        .await
        .try_into_stream()
        .map(|_| "stream");
    assert_err(result, "unknown(): Unknown function");
}

/// Render to a byte stream, but there's an error while collecting one of the
/// streams
#[tokio::test]
async fn test_render_stream_collect_error() {
    let template: Template = "{{ file('fake.txt') }}".into();
    let context = TestContext { can_stream: true };
    let stream = template.render(&context).await.try_into_stream().unwrap();
    assert_err(
        stream.try_collect::<BytesMut>().await,
        if cfg!(unix) {
            "No such file or directory"
        } else {
            "The system cannot find the file specified"
        },
    );
}

/// Convert JSON values to template values
#[rstest]
#[case::null(serde_json::Value::Null, Value::Null)]
#[case::bool_true(serde_json::Value::Bool(true), Value::Boolean(true))]
#[case::bool_false(serde_json::Value::Bool(false), Value::Boolean(false))]
#[case::number_positive_int(serde_json::json!(42), Value::Integer(42))]
#[case::number_negative_int(serde_json::json!(-17), Value::Integer(-17))]
#[case::number_zero(serde_json::json!(0), Value::Integer(0))]
#[case::number_float(serde_json::json!(1.23), Value::Float(1.23))]
#[case::number_negative_float(serde_json::json!(-2.5), Value::Float(-2.5))]
#[case::number_zero_float(serde_json::json!(0.0), Value::Float(0.0))]
#[case::string_empty(serde_json::json!(""), "".into())]
#[case::string_simple(serde_json::json!("hello"), "hello".into())]
#[case::string_with_spaces(serde_json::json!("hello world"), "hello world".into())]
#[case::string_with_unicode(serde_json::json!("héllo 🌍"), "héllo 🌍".into())]
#[case::string_with_escapes(serde_json::json!("line1\nline2\ttab"), "line1\nline2\ttab".into())]
#[case::array(
    serde_json::json!([null, true, 42, "hello"]),
    Value::Array(vec![
        Value::Null,
        Value::Boolean(true),
        Value::Integer(42),
        "hello".into(),
    ])
)]
// Array of numbers should *not* be interpreted as bytes
#[case::array_numbers(serde_json::json!([1, 2, 3]), vec![1, 2, 3].into())]
#[case::array_nested(
    serde_json::json!([[1, 2], [3, 4]]),
    vec![Value::from(vec![1, 2]), Value::from(vec![3, 4])].into()
)]
#[case::object(
    serde_json::json!({"name": "John", "age": 30, "active": true}),
    Value::Object(indexmap! {
        "name".into() => "John".into(),
        "age".into() => Value::Integer(30),
        "active".into() => Value::Boolean(true),
    })
)]
#[case::object_nested(
    serde_json::json!({"user": {"name": "Alice", "scores": [95, 87]}}),
    Value::Object(indexmap! {
        "user".into() => Value::Object(indexmap! {
            "name".into() => "Alice".into(),
            "scores".into() =>
                Value::Array(vec![Value::Integer(95), Value::Integer(87)]),
        })
    })
)]
fn test_from_json(#[case] json: serde_json::Value, #[case] expected: Value) {
    let actual = Value::from_json(json);
    assert_eq!(actual, expected);
}

#[rstest]
#[case::one_arg("{{ 1 | identity() }}", "1")]
#[case::multiple_args("{{ 'cd' | concat('ab') }}", "abcd")]
// Piped value is the last positional arg, before kwargs
#[case::kwargs("{{ 'cd' | concat('ab', reverse=true) }}", "dcba")]
#[tokio::test]
async fn test_pipe(#[case] template: Template, #[case] expected: &str) {
    assert_eq!(
        template
            .render_string(&TestContext::default())
            .await
            .unwrap(),
        expected
    );
}

/// Test error context on a variety of error cases in function calls
#[rstest]
#[case::unknown_function("{{ fake() }}", "fake(): Unknown function")]
#[case::extra_arg(
    "{{ identity('a', 'b') }}",
    "identity(): Extra arguments 'b'"
)]
#[case::missing_arg("{{ add(1) }}", "add(): Not enough arguments")]
#[case::arg_render(
    // Argument fails to render
    "{{ add(f(), 2) }}",
    "add(): argument 0=f(): f(): Unknown function"
)]
#[case::arg_convert(
    // Argument renders but doesn't convert to what the func wants
    "{{ add(1, 'b') }}",
    "add(): argument 1='b': Expected integer"
)]
#[tokio::test]
async fn test_function_error(
    #[case] template: Template,
    #[case] expected_error: &str,
) {
    assert_err(
        template.render_string(&TestContext::default()).await,
        expected_error,
    );
}

#[derive(Debug, Default)]
struct TestContext {
    can_stream: bool,
}

impl Context for TestContext {
    fn can_stream(&self) -> bool {
        self.can_stream
    }

    async fn get_field(
        &self,
        identifier: &Identifier,
    ) -> Result<LazyValue, RenderError> {
        match identifier.as_str() {
            "name" => Ok("Mike".into()),
            "array" => Ok(vec!["a", "b", "c"].into()),
            "invalid_utf8" => Ok(b"\xc3\x28".into()),
            _ => Err(RenderError::FieldUnknown {
                field: identifier.clone(),
            }),
        }
    }

    async fn call(
        &self,
        function_name: &Identifier,
        mut arguments: Arguments<'_, Self>,
    ) -> Result<LazyValue, RenderError> {
        match function_name.as_str() {
            "identity" => {
                let value: Value = arguments.pop_position()?;
                arguments.ensure_consumed()?;
                Ok(value.into())
            }
            "add" => {
                let a: i64 = arguments.pop_position()?;
                let b: i64 = arguments.pop_position()?;
                arguments.ensure_consumed()?;
                Ok((a + b).into())
            }
            "concat" => {
                let mut a: String = arguments.pop_position()?;
                let b: String = arguments.pop_position()?;
                let reverse: bool = arguments.pop_keyword("reverse")?;
                arguments.ensure_consumed()?;
                a.push_str(&b);
                if reverse {
                    Ok(a.chars().rev().collect::<String>().into())
                } else {
                    Ok(a.into())
                }
            }
            "file" => {
                let file_name: String = arguments.pop_position()?;
                arguments.ensure_consumed()?;
                let path = test_data_dir().join(file_name);
                // Create a stream that first opens the file, then reads from it
                let stream = File::open(path.clone())
                    .map_ok(ReaderStream::new)
                    .try_flatten_stream()
                    .map_err(RenderError::other)
                    .boxed();
                Ok(LazyValue::Stream {
                    source: StreamSource::File { path },
                    stream,
                })
            }
            _ => Err(RenderError::FunctionUnknown),
        }
    }
}
