use crate::state::AppState;
use anyhow::Context;
use derive_more::{Deref, Display};
use liquid::ParserBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, Deserialize)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug, Serialize)]
pub struct TemplateValues<'a> {
    environment: Option<&'a HashMap<String, String>>,
}

impl TemplateString {
    /// Render the template string using values from the given state. We need
    /// the whole state so we can dynamically access the environment, responses,
    /// etc.
    pub fn render(&self, values: &TemplateValues) -> anyhow::Result<String> {
        // TODO make parser available statically
        // TODO cache built template (maybe just do it during startup?)
        let template = ParserBuilder::with_stdlib().build()?.parse(&self.0)?;
        // TODO implement ObjectView for TemplateValues
        let globals = liquid::to_object(&values.environment)?;
        template.render(&globals).with_context(|| {
            format!("Error rendering template string {:?}", self.0)
        })
    }
}

impl<'a> From<&'a AppState> for TemplateValues<'a> {
    fn from(state: &'a AppState) -> Self {
        Self {
            environment: state.environments.selected().map(|e| &e.data),
        }
    }
}
