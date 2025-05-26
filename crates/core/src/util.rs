//! Miscellaneous utility constants/types/functions

use derive_more::Display;
use dialoguer::Confirm;
use std::fmt;

/// Link to the GitHub New Issue form
pub const NEW_ISSUE_LINK: &str =
    "https://github.com/LucasPickering/slumber/issues/new/choose";

/// Get a link to a page on the doc website. This will append the doc prefix,
/// as well as the suffix.
///
/// ```
/// use slumber_core::util::doc_link;
/// assert_eq!(
///     doc_link("api/chain"),
///     "https://slumber.lucaspickering.me/book/api/chain.html",
/// );
/// ```
pub fn doc_link(path: &str) -> String {
    const ROOT: &str = "https://slumber.lucaspickering.me/book/";
    if path.is_empty() {
        ROOT.into()
    } else {
        format!("{ROOT}{path}.html")
    }
}

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
