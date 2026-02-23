//! Miscellaneous utility constants/types/functions

use dialoguer::Confirm;
use slumber_template::Value;
use std::fmt::{self, Display};

/// Show the user a confirmation prompt
pub fn confirm(prompt: impl Into<String>) -> bool {
    Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .wait_for_newline(true)
        .interact()
        .unwrap_or(false)
}

/// Convert a template [Value] to a JSON value
pub fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => b.into(),
        Value::Integer(i) => i.into(),
        Value::Float(f) => f.into(),
        Value::String(s) => s.into(),
        Value::Array(array) => array.into_iter().map(value_to_json).collect(),
        Value::Object(object) => object
            .into_iter()
            .map(|(key, value)| (key, value_to_json(value)))
            .collect(),
        // Convert bytes to an int array. This isn't really useful, but it
        // keeps this method infallible which is really nice. And generally
        // it will probably be less disruptive to the user than an error.
        Value::Bytes(bytes) => bytes.to_vec().into(),
    }
}

/// Helper to printing bytes. If the bytes aren't valid UTF-8, they'll be
/// printed in hex representation instead
pub struct MaybeStr<'a>(pub &'a [u8]);

impl Display for MaybeStr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(self.0) {
            write!(f, "{s}")
        } else {
            let bytes_per_line = 12;
            // Format raw bytes in pairs of bytes
            for (i, byte) in self.0.iter().enumerate() {
                if i > 0 {
                    // Add whitespace before this group. Only use line breaks
                    // in alternate mode
                    if f.alternate() && i % bytes_per_line == 0 {
                        writeln!(f)?;
                    } else {
                        write!(f, " ")?;
                    }
                }

                write!(f, "{byte:02x}")?;
            }
            Ok(())
        }
    }
}
