/// Serialize/deserialize a duration with unit shorthand. This does *not* handle
/// subsecond precision. Supported units are:
/// - s
/// - m
/// - h
/// - d
///
/// Examples: `30s`, `5m`, `12h`, `3d`
pub mod serde_duration {
    use derive_more::Display;
    use itertools::Itertools;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::time::Duration;
    use strum::{EnumIter, EnumString, IntoEnumIterator};
    use winnow::{PResult, Parser, ascii::digit1, token::take_while};

    #[derive(Debug, Display, EnumIter, EnumString)]
    enum Unit {
        #[display("s")]
        #[strum(serialize = "s")]
        Second,
        #[display("m")]
        #[strum(serialize = "m")]
        Minute,
        #[display("h")]
        #[strum(serialize = "h")]
        Hour,
        #[display("d")]
        #[strum(serialize = "d")]
        Day,
    }

    pub fn serialize<S>(
        duration: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Always serialize as seconds, because it's easiest. Sub-second
        // precision is lost
        S::serialize_str(serializer, &format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        fn quantity(input: &mut &str) -> PResult<u64> {
            digit1.parse_to().parse_next(input)
        }

        fn unit<'a>(input: &mut &'a str) -> PResult<&'a str> {
            take_while(1.., char::is_alphabetic).parse_next(input)
        }

        let input = String::deserialize(deserializer)?;
        let (quantity, unit) = (quantity, unit)
            .parse(&input)
            // The format is so simple there isn't much value in spitting out a
            // specific parsing error, just use a canned one
            .map_err(|_| {
                D::Error::custom(
                    "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)",
                )
            })?;

        let unit = unit.parse().map_err(|_| {
            D::Error::custom(format!(
                "Unknown duration unit `{unit}`; must be one of {}",
                Unit::iter()
                    .format_with(", ", |unit, f| f(&format_args!("`{unit}`")))
            ))
        })?;
        let seconds = match unit {
            Unit::Second => quantity,
            Unit::Minute => quantity * 60,
            Unit::Hour => quantity * 60 * 60,
            Unit::Day => quantity * 60 * 60 * 24,
        };
        Ok(Duration::from_secs(seconds))
    }
}
