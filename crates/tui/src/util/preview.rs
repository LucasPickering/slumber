use async_trait::async_trait;
use itertools::Itertools;
use slumber_core::{collection::JsonTemplate, render::TemplateContext};
use slumber_template::{RenderedOutput, Template};
use std::borrow::Cow;

/// TODO
/// TODO is there a better place for this to live?
#[async_trait(?Send)]
pub trait Preview {
    /// TODO
    fn display(&self) -> Cow<'_, str>;

    /// TODO
    fn is_dynamic(&self) -> bool;

    /// TODO
    async fn render(&self, context: &TemplateContext) -> RenderedOutput;
}

#[async_trait(?Send)]
impl Preview for Template {
    fn display(&self) -> Cow<'_, str> {
        self.display()
    }

    fn is_dynamic(&self) -> bool {
        self.is_dynamic()
    }

    async fn render(&self, context: &TemplateContext) -> RenderedOutput {
        self.render(context).await
    }
}

#[async_trait(?Send)]
impl Preview for JsonTemplate {
    fn display(&self) -> Cow<'_, str> {
        match self {
            Self::Null => "null".into(),
            Self::Bool(false) => "false".into(),
            Self::Bool(true) => "true".into(),
            Self::Number(number) => number.to_string().into(),
            Self::String(template) => {
                format!("\"{}\"", template.display()).into()
            }
            // TODO prettiness
            Self::Array(array) => {
                format!("[{}]", array.iter().map(Self::display).format(", "))
                    .into()
            }
            Self::Object(object) => format!(
                "{{{}}}",
                object.iter().format_with(", ", |(key, value), f| f(
                    &format_args!("\"{}\": {}", key.display(), value.display())
                ))
            )
            .into(),
        }
    }

    fn is_dynamic(&self) -> bool {
        match self {
            Self::Null | Self::Bool(_) | Self::Number(_) => false,
            Self::String(template) => template.is_dynamic(),
            Self::Array(array) => array.iter().any(Self::is_dynamic),
            Self::Object(object) => object
                .iter()
                .any(|(key, value)| key.is_dynamic() || value.is_dynamic()),
        }
    }

    async fn render(&self, context: &TemplateContext) -> RenderedOutput {
        self.render(context).await
    }
}

// TODO write tests for JSON formatting
