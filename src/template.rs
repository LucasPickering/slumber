use anyhow::Context;
use derive_more::{Deref, Display};
use liquid::ParserBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, Deserialize)]
pub struct TemplateString(String);

impl TemplateString {
    /// Render the template string using values from the given state. We need
    /// the whole state so we can dynamically access the environment, responses,
    /// etc.
    pub fn render(&self, values: &TemplateValues) -> anyhow::Result<String> {
        // TODO make parser available statically
        // TODO cache built template (maybe just do it during startup?)
        let template = ParserBuilder::with_stdlib().build()?.parse(&self.0)?;
        // TODO implement ObjectView for TemplateValues
        let empty_map = HashMap::new();
        let globals =
            liquid::to_object(values.environment.unwrap_or(&empty_map))?;
        template.render(&globals).with_context(|| {
            format!("Error rendering template string {:?}", self.0)
        })
    }
}

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug, Serialize)]
pub struct TemplateValues<'a> {
    /// Technically this could just be an empty hashmap instead of needing an
    /// option, but that makes it hard when the environment is missing on the
    /// creator's side, because they need to create an empty map and figure out
    /// how to keep it around
    pub environment: Option<&'a HashMap<String, String>>,
}
