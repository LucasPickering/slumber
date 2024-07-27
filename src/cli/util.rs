use dialoguer::console::Style;
use reqwest::header::HeaderMap;
use slumber_core::util::MaybeStr;
use std::fmt::{self, Display, Formatter};

/// Wrapper making it easy to print a header map
pub struct HeaderDisplay<'a>(pub &'a HeaderMap);

impl<'a> Display for HeaderDisplay<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let key_style = Style::new().bold();
        for (key, value) in self.0 {
            writeln!(
                f,
                "{}: {}",
                key_style.apply_to(key),
                MaybeStr(value.as_bytes()),
            )?;
        }
        Ok(())
    }
}
