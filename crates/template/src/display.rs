//! Template stringification

use crate::{
    Expression, Template, TemplateChunk,
    parse::{ESCAPE, EXPRESSION_CLOSE, EXPRESSION_OPEN},
};
use regex::Regex;
use std::{
    borrow::Cow,
    fmt::{self, Display, Write},
    sync::LazyLock,
};

impl Template {
    /// Convert the template to a string. This will only allocate for escaped or
    /// keyed templates. This is guaranteed to return the exact string that was
    /// parsed to create the template, and therefore will parse back to the same
    /// template. If it doesn't, that's a bug.
    pub fn display(&self) -> Cow<'_, str> {
        let mut buf = Cow::Borrowed("");

        // Re-stringify the template
        for chunk in &self.chunks {
            match chunk {
                TemplateChunk::Raw(s) => {
                    // Add underscores between { to escape them. Any sequence
                    // of {_* followed by another { needs to be escaped. Regex
                    // matches have to be non-overlapping so we can't just use
                    // {_*{, because that wouldn't catch cases like {_{_{. So
                    // we have to do our own lookahead.
                    //
                    // Keep in mind that escape sequences are going to be an
                    // extreme rarity, so we need to optimize for the case where
                    // there are none and only allocate when necessary.
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*").unwrap());
                    // Track how far into s we've copied, so we can do as few
                    // copies as possible
                    let mut last_copied = 0;
                    for m in REGEX.find_iter(s) {
                        let rest = &s[m.end()..];
                        // Don't allocate until we know this needs an escape
                        // sequence
                        if rest.starts_with('{') {
                            let buf = buf.to_mut();
                            buf.push_str(&s[last_copied..m.end()]);
                            buf.push('_');
                            last_copied = m.end();
                        }
                    }

                    // If this is the first chunk and there were no regex
                    // matches, don't allocate yet
                    if buf.is_empty() {
                        buf = Cow::Borrowed(s);
                    } else {
                        // Fencepost: get everything from the last escape
                        // sequence to the end
                        buf.to_mut().push_str(&s[last_copied..]);
                    }
                }
                TemplateChunk::Expression(expression) => {
                    // If the previous chunk ends with a potential escape
                    // sequence, add an underscore to escape the upcoming key
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*$").unwrap());
                    if REGEX.is_match(&buf) {
                        buf.to_mut().push_str(ESCAPE);
                    }

                    write!(
                        buf.to_mut(),
                        "{EXPRESSION_OPEN}{expression}{EXPRESSION_CLOSE}"
                    )
                    .unwrap();
                }
            }
        }

        buf
    }
}

impl Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}
