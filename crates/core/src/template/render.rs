//! Template rendering implementation

use crate::template::{Template, TemplateContext, TemplateError};
use hcl::eval::{Context, Evaluate};
use indexmap::{indexmap, IndexMap};

impl Template {
    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The template is rendered as bytes.
    /// Use [Self::render_string] if you want the bytes converted to a string.
    pub async fn render(
        &self,
        context: &TemplateContext,
    ) -> Result<Vec<u8>, TemplateError> {
        // TODO
        self.render_string(context).await.map(String::into_bytes)
    }

    /// Render the template using values from the given context. If any chunk
    /// failed to render, return an error. The rendered template will be
    /// converted from raw bytes to UTF-8. If it is not valid UTF-8, return an
    /// error.
    pub async fn render_string(
        &self,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        let mut hcl_context = Context::default();
        hcl_context.declare_var(
            "locals",
            hcl::Value::Object(indexmap! {
                "username".into() => "jumbo".into(),
                "password".into() => "hunter2".into(),
            }),
        );
        if let Some(profile) = context.profile() {
            // Each value is an expression itself
            let data = profile
                .data
                .iter()
                .map(|(field, value)| {
                    (
                        field.clone(),
                        value
                            .0
                            .evaluate(&hcl_context)
                            .expect("Error rendering inner TODO"),
                    )
                })
                .collect::<IndexMap<_, _>>();
            hcl_context.declare_var("profile", data);
        }
        let value =
            self.0.evaluate(&hcl_context).expect("Error rendering TODO");
        Ok(value
            .as_str()
            .expect("Did not render to string TODO")
            .to_owned())
    }
}
