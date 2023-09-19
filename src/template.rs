use derive_more::{Deref, Display};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::OnceLock};
use thiserror::Error;

static TEMPLATE_REGEX: OnceLock<Regex> = OnceLock::new();

/// A string that can contain templated content
#[derive(Clone, Debug, Deref, Display, Deserialize)]
pub struct TemplateString(String);

#[derive(Debug, Error)]
pub enum TemplateError {
    // TODO include span in errors
    // TODO try to make this work with lifetimes
    #[error("Unknown key {key:?} in {template:?}")]
    UnknownKey { template: String, key: String },
}

impl TemplateString {
    /// Render the template string using values from the given state. We need
    /// the whole state so we can dynamically access the environment, responses,
    /// etc.
    pub fn render(
        &self,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        // Template syntax is simple so it's easiest to just implement it with
        // a regex
        let re = TEMPLATE_REGEX
            .get_or_init(|| Regex::new(r"\{\{\s*([\w\d_-]+)\s*\}\}").unwrap());

        // Regex::replace_all doesn't support fallible replacement, so we
        // have to do it ourselves. Use a Cow so we don't allocate for
        // strings that contain no templating.
        // https://docs.rs/regex/1.9.5/regex/struct.Regex.html#method.replace_all
        let mut new = String::with_capacity(self.len());
        let mut last_match = 0;
        for captures in re.captures_iter(self) {
            let m = captures.get(0).unwrap();
            new.push_str(&self[last_match..m.start()]);
            let key =
                captures.get(1).expect("Missing key capture group").as_str();
            new.push_str(context.get(key).ok_or_else(|| {
                TemplateError::UnknownKey {
                    template: self.0.clone(),
                    key: key.to_owned(),
                }
            })?);
            last_match = m.end();
        }
        new.push_str(&self[last_match..]);

        Ok(new)
    }
}

/// A little container struct for all the data that the user can access via
/// templating. This is derived from AppState, and will only store references
/// to that state (without cloning).
#[derive(Debug, Serialize)]
pub struct TemplateContext<'a> {
    /// Technically this could just be an empty hashmap instead of needing an
    /// option, but that makes it hard when the environment is missing on the
    /// creator's side, because they need to create an empty map and figure out
    /// how to keep it around
    pub environment: Option<&'a HashMap<String, String>>,
    /// Additional key=value overrides passed directly from the user
    pub overrides: Option<&'a HashMap<String, String>>,
}

impl<'a> TemplateContext<'a> {
    /// Get a value by key
    fn get(&self, key: &str) -> Option<&String> {
        fn get_opt<'a>(
            map: Option<&'a HashMap<String, String>>,
            key: &str,
        ) -> Option<&'a String> {
            map?.get(key)
        }

        None.or_else(|| get_opt(self.overrides, key))
            .or_else(|| get_opt(self.environment, key))
    }
}

#[cfg(test)]
mod tests {
    use crate::template::{TemplateContext, TemplateString};

    #[test]
    fn test_valid_template() {
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
        };
        assert_eq!(
            TemplateString("".into()).render(&context).unwrap(),
            "".to_owned()
        );
        assert_eq!(
            TemplateString("plain".into()).render(&context).unwrap(),
            "plain".to_owned()
        );
        // Pull from overrides
        assert_eq!(
            TemplateString("{{user_id}}".into())
                .render(&context)
                .unwrap(),
            "2".to_owned()
        );
        // Pull from env
        assert_eq!(
            TemplateString("{{group_id}}".into())
                .render(&context)
                .unwrap(),
            "3".to_owned()
        );
    }

    #[test]
    fn test_unknown_key() {
        let context = TemplateContext {
            environment: None,
            overrides: None,
        };
        assert_eq!(
            TemplateString("{{user_id}}".into())
                .render(&context)
                .unwrap_err()
                .to_string(),
            "Unknown key \"user_id\" in \"{{user_id}}\"".to_owned()
        );
    }
}
