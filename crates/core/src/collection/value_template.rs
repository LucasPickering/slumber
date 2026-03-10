use futures::future;
use serde::Serialize;
use slumber_template::{
    Context, RenderError, RenderValue, RenderedChunks, Template, Value,
};

/// A templated [Value]
///
/// This is a [Value], except the strings are templates. That means this can be
/// dynamically rendered to a [Value]. This is used for structured bodies (e.g.
/// JSON) as well as profile fields.
#[derive(Clone, Debug, derive_more::From, PartialEq, Serialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ValueTemplate {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(Template),
    #[from(ignore)]
    Array(Vec<Self>),
    // A key-value mapping. Stored as a `Vec` instead of `IndexMap` because
    // the keys are templates, which aren't hashable. We never do key lookups
    // on this so there's no need for a map anyway.
    #[from(ignore)]
    #[serde(serialize_with = "slumber_util::serialize_mapping")]
    #[cfg_attr(
        feature = "schema",
        schemars(with = "std::collections::HashMap<Template, Self>")
    )]
    Object(Vec<(Template, Self)>),
}

impl ValueTemplate {
    /// Create a new string template from a raw string, without parsing it at
    /// all
    ///
    /// Useful when importing from external formats where the string isn't
    /// expected to be a valid Slumber template
    pub fn raw(template: String) -> Self {
        Self::String(Template::raw(template))
    }

    /// Does the template have at least one dynamic chunk? If this returns
    /// `false`, the template will always render to its source text
    pub fn is_dynamic(&self) -> bool {
        match self {
            Self::Null
            | Self::Boolean(_)
            | Self::Integer(_)
            | Self::Float(_) => false,
            Self::String(template) => template.is_dynamic(),
            Self::Array(array) => array.iter().any(Self::is_dynamic),
            Self::Object(object) => object
                .iter()
                .any(|(key, value)| key.is_dynamic() || value.is_dynamic()),
        }
    }

    /// Render to previewable chunks
    ///
    /// The return value is *usually* a single chunk, but if `self` is a
    /// multi-chunk template string, then its multi-chunk output will be the
    /// output for this.
    ///
    /// TODO explain type param
    pub async fn render<Ctx, V>(&self, context: &Ctx) -> RenderedChunks<V>
    where
        Ctx: Context,
        V: From<Value> + RenderValue,
    {
        match self {
            Self::Null => Value::Null.into(),
            Self::Boolean(b) => Value::Boolean(*b).into(),
            Self::Integer(i) => Value::Integer(*i).into(),
            Self::Float(f) => Value::Float(*f).into(),
            Self::String(template) => template.render(context).await,
            Self::Array(array) => {
                // Render each value and collection into an Array
                future::try_join_all(array.iter().map(|value| async {
                    value
                        .render::<Ctx, V>(context)
                        .await
                        .try_collect_value()
                        .await
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedOutput
            }
            Self::Object(map) => {
                // Render each key/value and collect into an Object
                future::try_join_all(map.iter().map(|(key, value)| async {
                    let key = key.render_string(context).await?;
                    let value = value
                        .render::<Ctx, V>(context)
                        .await
                        .try_collect_value()
                        .await?;
                    Ok::<_, RenderError>((key, value))
                }))
                .await
                .map(Value::from)
                .into() // Wrap into RenderedOutput
            }
        }
    }
}

/// Parse template from a string literal. Panic if invalid
#[cfg(any(test, feature = "test"))]
impl From<&str> for ValueTemplate {
    fn from(value: &str) -> Self {
        let template = value.parse().unwrap();
        Self::String(template)
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<T>> for ValueTemplate {
    fn from(value: Vec<T>) -> Self {
        Self::Array(value.into_iter().map(T::into).collect())
    }
}

#[cfg(any(test, feature = "test"))]
impl<T: Into<ValueTemplate>> From<Vec<(&str, T)>> for ValueTemplate {
    fn from(value: Vec<(&str, T)>) -> Self {
        Self::Object(
            value
                .into_iter()
                .map(|(k, v)| (k.parse().unwrap(), v.into()))
                .collect(),
        )
    }
}
