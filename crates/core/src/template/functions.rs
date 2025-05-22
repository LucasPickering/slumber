//! Custom minijinja functions

use crate::template::{Prompt, TemplateContext, error::FunctionError};
use minijinja::{
    Environment, Error, ErrorKind, State, Value,
    value::{Kwargs, ViaDeserialize},
};
use serde_json_path::JsonPath;
use std::{fs, sync::Arc};
use tokio::{runtime::Handle, sync::oneshot};

// TODO statically typed kwargs

/// Register all slumber functions in the environment
pub fn add_functions(environment: &mut Environment) {
    // TODO the rest of these
    // exports.export_fn("command", sync(command));
    // exports.export_fn("response", sync(response));
    // exports.export_fn("responseHeader", sync(response_header));
    // exports.export_fn("select", sync(select));

    // Filters
    environment.add_filter("jsonpath", jsonpath);
    environment.add_filter("sensitive", sensitive);

    // Functions
    environment.add_function("file", file);
    environment.add_function("prompt", prompt);
}

/// TODO
fn file(path: &str) -> Binary {
    Binary(fs::read(path).expect("TODO"))
}

/// Transform a JSON value using a JSONPath query
fn jsonpath(
    ViaDeserialize(query): ViaDeserialize<JsonPath>,
    ViaDeserialize(value): ViaDeserialize<serde_json::Value>,
) -> Result<Value, Error> {
    // TODO support mode?
    let node_list = query.query(&value);
    // TODO can we avoid this collection? is it possible to go NodeList straight
    // to Value? what serializer/deserialize do we use?
    let json: serde_json::Value = node_list.into_iter().cloned().collect();
    // Convert from JSON to minijinja::Value
    Ok(serde_json::from_value(json).unwrap())
}

/// TODO
fn prompt(state: &State, kwargs: Kwargs) -> Result<String, Error> {
    block_on(async {
        let context = context(state)?;
        // TODO static kwargs
        let message: Option<String> = kwargs.get("message")?;
        let default: Option<String> = kwargs.get("default")?;
        let sensitive: bool =
            kwargs.get::<Option<bool>>("sensitive")?.unwrap_or(false);
        let (tx, rx) = oneshot::channel();
        context.prompter.prompt(Prompt {
            message: message.unwrap_or_default(),
            default,
            sensitive,
            channel: tx.into(),
        });
        let output = rx.await.map_err(|_| FunctionError::PromptNoReply)?;

        // If the input was sensitive, we should mask it for previews as well.
        // This is a little wonky because the preview prompter just spits out a
        // static string anyway, but it's "technically" right and plays well in
        // tests. Also it reminds users that a prompt is sensitive in the TUI :)
        if sensitive {
            Ok(mask_sensitive(&context, output))
        } else {
            Ok(output)
        }
    })
}

/// TODO
fn sensitive(state: &State, value: String) -> Result<String, Error> {
    let context = context(state)?;
    Ok(mask_sensitive(&context, value))
}

/// TODO
struct Binary(Vec<u8>);

impl From<Binary> for Value {
    fn from(value: Binary) -> Self {
        Value::from_bytes(value.0)
    }
}

/// Run a future to completion on the current tokio runtime. This is used to
/// wrap async code to be sync so it can be used in a minijinja template
/// function.
///
/// This needs to be called within the defined function. The generics involved
/// to wrap the entire function at the register site (`add_function` or
/// `add_filter`) are much more complicated.
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let rt = Handle::current();
    rt.block_on(future)
}

/// Get template context from the render state
fn context(state: &State) -> Result<Arc<TemplateContext>, Error> {
    state
        .lookup(TemplateContext::SELF_KEY)
        .and_then(|x| x.downcast_object())
        .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "TODO"))
}

/// Hide a sensitive value if the context has show_sensitive disabled
fn mask_sensitive(context: &TemplateContext, value: String) -> String {
    if context.show_sensitive {
        value
    } else {
        "•".repeat(value.chars().count())
    }
}
