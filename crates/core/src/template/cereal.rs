use crate::template::Template;
use serde::{
    de::{Error, Visitor},
    Deserialize, Deserializer, Serialize,
};

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_test::{assert_de_tokens, Token};

    /// Test deserialization, which has some additional logic on top of parsing
    #[rstest]
    // boolean
    #[case::bool_true(Token::Bool(true), "true")]
    #[case::bool_false(Token::Bool(false), "false")]
    // numeric
    #[case::u64(Token::U64(1000), "1000")]
    #[case::i64_negative(Token::I64(-1000), "-1000")]
    #[case::float_positive(Token::F64(10.1), "10.1")]
    #[case::float_negative(Token::F64(-10.1), "-10.1")]
    // string
    #[case::str(Token::Str("hello"), "hello")]
    #[case::str_null(Token::Str("null"), "null")]
    #[case::str_true(Token::Str("true"), "true")]
    #[case::str_false(Token::Str("false"), "false")]
    #[case::str_with_keys(Token::Str("{{user_id}}"), "{{user_id}}")]
    fn test_deserialize(#[case] token: Token, #[case] expected: &str) {
        assert_de_tokens(&Template::from(expected), &[token]);
    }
}
