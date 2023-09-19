use crate::{config::Chain, history::RequestHistory, util::ResultExt};
use anyhow::Context;
use derive_more::{Deref, Display};
use regex::Regex;
use serde::Deserialize;
use std::{borrow::Cow, collections::HashMap, ops::Deref as _, sync::OnceLock};
use thiserror::Error;
use tracing::{instrument, trace};

static TEMPLATE_REGEX: OnceLock<Regex> = OnceLock::new();

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, Deserialize)]
pub struct TemplateString(String);

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug)]
pub struct TemplateContext<'a> {
    /// Technically this could just be an empty hashmap instead of needing an
    /// option, but that makes it hard when the environment is missing on the
    /// creator's side, because they need to create an empty map and figure out
    /// how to keep it around
    pub environment: Option<&'a HashMap<String, String>>,
    pub chains: &'a [Chain],
    pub history: &'a RequestHistory,
    /// Additional key=value overrides passed directly from the user
    pub overrides: Option<&'a HashMap<String, String>>,
}

/// Any error that can occur during template rendering. Generally the generic
/// parameter will be either `&str` (for localized errors) or `String` (for
/// global errors that need to be propagated up).
///
/// The purpose of having a structured error here (while the rest of the app
/// just uses `anyhow`) is to support localized error display in the UI, e.g.
/// showing just one portion of a string in red if that particular template
/// key failed to render.
#[derive(Debug, Error)]
pub enum TemplateError<S: std::fmt::Display> {
    /// Template key could not be parsed
    #[error("Failed to parse template key {key:?}")]
    InvalidKey { key: S },

    /// A basic field key contained an unknown field
    #[error("Unknown field {field:?}")]
    UnknownField { field: S },

    #[error("Unknown chain {chain_id:?}")]
    UnknownChain { chain_id: S },

    /// The chain ID is valid, but the corresponding recipe has no successful
    /// response
    #[error("No response available for chain {chain_id:?}")]
    NoChainResponse { chain_id: S },

    /// An error occurred accessing history
    #[error("{0}")]
    History(#[source] anyhow::Error),
}

impl TemplateString {
    /// Render the template string using values from the given context. If an
    /// error occurs, it is returned as general `anyhow` error. If you need a
    /// more specific error, use [Self::render_borrow].
    pub fn render(&self, context: &TemplateContext) -> anyhow::Result<String> {
        self.render_borrow(context)
            .map_err(TemplateError::into_owned)
            .with_context(|| format!("Error rendering template {:?}", self.0))
            .traced()
    }

    /// Render the template string using values from the given state. If an
    /// error occurs, return a borrowed error type that references the template
    /// string. Useful for inline rendering in the UI.
    #[instrument]
    pub fn render_borrow<'a>(
        &'a self,
        context: &'a TemplateContext,
    ) -> Result<String, TemplateError<&'a str>> {
        // Template syntax is simple so it's easiest to just implement it with
        // a regex
        let re = TEMPLATE_REGEX
            .get_or_init(|| Regex::new(r"\{\{\s*([\w\d._-]+)\s*\}\}").unwrap());

        // Regex::replace_all doesn't support fallible replacement, so we
        // have to do it ourselves.
        // https://docs.rs/regex/1.9.5/regex/struct.Regex.html#method.replace_all

        let mut new = String::with_capacity(self.len());
        let mut last_match = 0;
        for captures in re.captures_iter(self) {
            let m = captures.get(0).unwrap();
            new.push_str(&self[last_match..m.start()]);
            let key_raw =
                captures.get(1).expect("Missing key capture group").as_str();
            let key = TemplateKey::parse(key_raw)?;
            let rendered_value = context.get(key)?;
            trace!(
                key = key_raw,
                value = rendered_value.deref(),
                "Rendered template key"
            );
            // Replace the key with its value
            new.push_str(&rendered_value);
            last_match = m.end();
        }
        new.push_str(&self[last_match..]);

        Ok(new)
    }
}

impl<'a> TemplateContext<'a> {
    /// Get a value by key
    fn get(
        &self,
        key: TemplateKey<'a>,
    ) -> Result<Cow<'a, str>, TemplateError<&'a str>> {
        fn get_opt<'a>(
            map: Option<&'a HashMap<String, String>>,
            key: &str,
        ) -> Option<&'a String> {
            map?.get(key)
        }

        match key {
            // Plain fields
            TemplateKey::Field(field) => None
                // Cascade down the the list of maps we want to check
                .or_else(|| get_opt(self.overrides, field))
                .or_else(|| get_opt(self.environment, field))
                .map(Cow::from)
                .ok_or(TemplateError::UnknownField { field }),

            // Chained response values
            TemplateKey::Chain(chain_id) => {
                // Resolve chained value
                let chain = self
                    .chains
                    .iter()
                    .find(|chain| chain.id == chain_id)
                    .ok_or(TemplateError::UnknownChain { chain_id })?;
                let response = self
                    .history
                    .get_last_success(&chain.source)
                    .map_err(TemplateError::History)?
                    .ok_or(TemplateError::NoChainResponse { chain_id })?;

                // TODO support jsonpath
                Ok(response.content.into())
            }
        }
    }
}

impl<'a> TemplateError<&'a str> {
    /// Convert a borrowed error into an owned one by cloning every string
    pub fn into_owned(self) -> TemplateError<String> {
        match self {
            Self::InvalidKey { key } => TemplateError::InvalidKey {
                key: key.to_owned(),
            },
            Self::UnknownField { field } => TemplateError::UnknownField {
                field: field.to_owned(),
            },
            Self::UnknownChain { chain_id } => TemplateError::UnknownChain {
                chain_id: chain_id.to_owned(),
            },
            Self::NoChainResponse { chain_id } => {
                TemplateError::NoChainResponse {
                    chain_id: chain_id.to_owned(),
                }
            }
            Self::History(err) => TemplateError::History(err),
        }
    }
}

/// A parsed template key. The variant of this determines how the key will be
/// resolved into a value.
#[derive(Clone, Debug, PartialEq)]
enum TemplateKey<'a> {
    /// A plain field, which can come from the environment or an override
    Field(&'a str),
    /// A value chained from the response of another recipe
    Chain(&'a str),
}

impl<'a> TemplateKey<'a> {
    /// Parse a string into a key. It'd be nice if this was a `FromStr`
    /// implementation, but that doesn't allow us to attach to the lifetime of
    /// the input `str`.
    fn parse(s: &'a str) -> Result<Self, TemplateError<&'a str>> {
        match s.split('.').collect::<Vec<_>>().as_slice() {
            [key] => Ok(Self::Field(key)),
            ["chains", chain_id] => Ok(Self::Chain(chain_id)),
            _ => Err(TemplateError::InvalidKey { key: s }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::RequestRecipeId, factory::*, util::assert_err};
    use anyhow::anyhow;
    use factori::create;

    /// Test that a field key renders correctly
    #[test]
    fn test_field() {
        let environment = [
            ("user_id".into(), "1".into()),
            ("group_id".into(), "3".into()),
        ]
        .into_iter()
        .collect();
        let overrides = [("user_id".into(), "2".into())].into_iter().collect();
        let context = TemplateContext {
            environment: Some(&environment),
            overrides: Some(&overrides),
            history: &RequestHistory::testing(),
            chains: &[],
        };

        // Success cases
        assert_eq!(
            TemplateString("".into()).render_borrow(&context).unwrap(),
            "".to_owned()
        );
        assert_eq!(
            TemplateString("plain".into())
                .render_borrow(&context)
                .unwrap(),
            "plain".to_owned()
        );
        assert_eq!(
            // Pull from overrides for user_id, env for group_id
            TemplateString("{{user_id}} {{group_id}}".into())
                .render_borrow(&context)
                .unwrap(),
            "2 3".to_owned()
        );

        // Error cases
        assert_err!(
            TemplateString("{{onion_id}}".into()).render_borrow(&context),
            "Unknown field \"onion_id\""
        );
    }

    #[test]
    fn test_chain() {
        let success_recipe_id = RequestRecipeId::from("success".to_string());
        let error_recipe_id = RequestRecipeId::from("error".to_string());
        let history = RequestHistory::testing();
        history.add(
            &create!(Request, recipe_id: success_recipe_id.clone()),
            &Ok(create!(Response, content: "Hello World!".into())),
        );
        history.add(
            &create!(Request, recipe_id: error_recipe_id.clone()),
            &Err(anyhow!("Something went wrong!")),
        );
        let context = TemplateContext {
            environment: None,
            overrides: None,
            history: &history,
            chains: &[
                create!(Chain, id: "chain1".into(), source: success_recipe_id),
                create!(Chain, id: "chain2".into(), source: error_recipe_id),
                create!(Chain, id: "chain3".into(), source: "unknown".to_owned().into()),
            ],
        };

        // Success cases
        assert_eq!(
            TemplateString("{{chains.chain1}}".into())
                .render_borrow(&context)
                .unwrap(),
            "Hello World!"
        );

        // Error cases
        assert_err!(
            // Unknown chain
            TemplateString("{{chains.unknown}}".into()).render_borrow(&context),
            "Unknown chain \"unknown\""
        );
        assert_err!(
            // Chain is known, but has no success response
            TemplateString("{{chains.chain2}}".into()).render_borrow(&context),
            "No response available for chain \"chain2\""
        );
        assert_err!(
            // Chain is known, but its recipe isn't
            TemplateString("{{chains.chain3}}".into()).render_borrow(&context),
            "No response available for chain \"chain3\""
        );
    }

    /// Test parsing just *inside* the {{ }}
    #[test]
    fn test_parse_template_key_success() {
        // Success cases
        assert_eq!(
            TemplateKey::parse("field_id").unwrap(),
            TemplateKey::Field("field_id")
        );
        assert_eq!(
            TemplateKey::parse("chains.chain_id").unwrap(),
            TemplateKey::Chain("chain_id")
        );
        // This is "valid", but probably won't match anything
        assert_eq!(
            TemplateKey::parse("chains.").unwrap(),
            TemplateKey::Chain("")
        );

        // Error cases
        assert_err!(
            TemplateKey::parse("."),
            "Failed to parse template key \".\""
        );
        assert_err!(
            TemplateKey::parse(".bad"),
            "Failed to parse template key \".bad\""
        );
        assert_err!(
            TemplateKey::parse("bad."),
            "Failed to parse template key \"bad.\""
        );
        assert_err!(
            TemplateKey::parse("chains.good.bad"),
            "Failed to parse template key \"chains.good.bad\""
        );
    }
}
