use crate::state::AppState;
use anyhow::Context;
use derive_more::{Deref, Display};
use leon::{Template, Values};
use serde::Deserialize;
use std::{borrow::Cow, collections::HashMap};

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, Deserialize)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug)]
pub struct TemplateValues<'a> {
    environment: Option<&'a HashMap<String, String>>,
}

impl TemplateString {
    /// Render the template string using values from the given state. We need
    /// the whole state so we can dynamically access the environment, responses,
    /// etc.
    pub fn render(&self, values: &TemplateValues) -> anyhow::Result<String> {
        Template::parse(&self.0)?.render(values).with_context(|| {
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

/// Tell leon how to fetch template values
impl<'a> Values for TemplateValues<'a> {
    fn get_value(&self, key: &str) -> Option<Cow<'_, str>> {
        self.environment?.get(key).map(Into::into)
    }
}
