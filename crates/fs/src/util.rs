use mime::Mime;

/// Get a file extension for a MIME type
///
/// If the MIME type is `None` or unknown, return `"data"`
/// TODO this should respect MIME overrides from the config
pub fn mime_to_extension(mime: Option<&Mime>) -> &'static str {
    use mime::{APPLICATION, JSON, TEXT, XML};
    const DEFAULT: &str = "data"; // Everything is data, right?

    // This is duplicated from the TUI matching logic because I didn't want to
    // tie this to syntax highlighting or other language support in the TUI
    let Some(mime) = mime else { return DEFAULT };
    match (mime.type_(), mime.subtype()) {
        // TODO match shit like `application/foo+json`
        (APPLICATION, JSON) => "json",
        (TEXT, XML) => "xml",
        (TEXT, _) => "txt",
        _ => DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Test MIME -> file extension mapping
    #[rstest]
    #[case::json("application/json", "json")]
    // TODO add more cases
    #[case::unknown("unknown/unknown", "data")]
    fn test_mime_to_extension(#[case] mime: Mime, #[case] expected: &str) {
        assert_eq!(mime_to_extension(Some(&mime)), expected);
    }
}
