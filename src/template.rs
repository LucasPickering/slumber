use anyhow::Context;
use leon::Template;
use serde::Deserialize;
use std::{collections::HashMap, fmt::Display};

/// A string that can contain templated content
#[derive(Clone, Debug, Deserialize)]
pub struct TemplateString(String);

impl TemplateString {
    /// Render the template string using the given value mapping
    pub fn render(
        &self,
        environment: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        Template::parse(&self.0)?
            .render(environment)
            .with_context(|| {
                format!("Error rendering template string {:?}", self.0)
            })
    }
}

impl Display for TemplateString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
