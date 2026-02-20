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

/// TODO
pub fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Float(_) => todo!(),
        Value::String(s) => serde_json::Value::String(s),
        Value::Array(array) => serde_json::Value::Array(
            array.into_iter().map(value_to_json).collect(),
        ),
        Value::Object(_) => todo!(),
        Value::Bytes(_) => todo!(),
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
