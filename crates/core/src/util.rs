//! Miscellaneous utility constants/types/functions

use derive_more::Display;
use dialoguer::Confirm;
use std::fmt;

/// Show the user a confirmation prompt
pub fn confirm(prompt: impl Into<String>) -> bool {
    Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .wait_for_newline(true)
        .interact()
        .unwrap_or(false)
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
