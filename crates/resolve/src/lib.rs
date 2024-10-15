mod parse;

use crate::parse::parse_reference;
use serde_yaml::Value;
use std::{borrow::Cow, collections::HashMap, fmt::Display, str::FromStr};

pub const REFERENCE_TAG: &str = "ref";

/// TODO
pub trait ResolveReferences {
    fn resolve_references(&mut self) -> Result<(), ReferenceError>;
}

impl ResolveReferences for Value {
    fn resolve_references(&mut self) -> Result<(), ReferenceError> {
        Resolver::default().resolve_all(self)
    }
}

/// TODO
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum ReferenceError {
    NotAReference(Value),
    InvalidReference(String),
    NoResource(Reference),
    CircularReference, // TODO store some data
}

impl Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReferenceError::NotAReference(value) => {
                write!(f, "Not a reference: {value}")
            }
            ReferenceError::InvalidReference(s) => {
                write!(f, "Invalid reference: {s}")
            }
            ReferenceError::NoResource(reference) => {
                write!(f, "Resource does not exist: {reference}")
            }
            ReferenceError::CircularReference => {
                write!(f, "TODO")
            }
        }
    }
}

impl Error for ReferenceError {}

#[derive(Default)]
struct Resolver {
    resolved: HashMap<Reference, Value>,
}

impl Resolver {
    fn resolve_all(mut self, value: &mut Value) -> Result<(), ReferenceError> {
        self.load_references(value, value)?;
        self.replace_references(value)?;
        Ok(())
    }

    /// Traverse the YAML value and for any reference, resolve its corresponding
    /// value and insert that value into the resolved map.
    fn load_references(
        &mut self,
        root: &Value,
        value: &Value,
    ) -> Result<(), ReferenceError> {
        if let Some(reference) = Reference::try_from_yaml(value)? {
            let value = reference.lookup(root, &reference.path)?;
            self.resolved.insert(reference, value.clone());
            // TODO recursion
        } else {
            match value {
                Value::Null
                | Value::Bool(_)
                | Value::Number(_)
                | Value::String(_) => {}
                Value::Sequence(vec) => {
                    for value in vec {
                        self.load_references(root, value)?;
                    }
                }
                Value::Mapping(mapping) => {
                    // TODO do keys too?
                    for value in mapping.values() {
                        self.load_references(root, value)?;
                    }
                }
                Value::Tagged(tagged_value) => {
                    self.load_references(root, &tagged_value.value)?;
                }
            }
        }
        Ok(())
    }

    fn replace_references(
        &self,
        value: &mut Value,
    ) -> Result<(), ReferenceError> {
        if let Some(reference) = Reference::try_from_yaml(value)? {
            let referenced =
                self.resolved.get(&reference).expect("TODO").clone();
            *value = referenced;
        } else {
            match value {
                Value::Null
                | Value::Bool(_)
                | Value::Number(_)
                | Value::String(_) => {}
                Value::Sequence(vec) => {
                    for value in vec {
                        self.replace_references(value)?;
                    }
                }
                Value::Mapping(mapping) => {
                    // TODO do keys too?
                    for value in mapping.values_mut() {
                        self.replace_references(value)?;
                    }
                }
                Value::Tagged(tagged_value) => {
                    self.replace_references(&mut tagged_value.value)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Reference {
    source: ReferenceSource,
    path: ReferencePath<'static>,
}

impl Reference {
    /// Attempt to parse a YAML value as a reference. Return `Ok(None)` if it's
    /// not a reference. Return `Ok(Some(_))` for a valid reference. Return
    /// `Err(_)` if it's tagged like a reference but isn't valid.
    fn try_from_yaml(value: &Value) -> Result<Option<Self>, ReferenceError> {
        match value {
            Value::Tagged(tagged_value)
                if tagged_value.tag == REFERENCE_TAG =>
            {
                // We can hit two error cases:
                // - It's a string but can't parse into a reference
                // - It's not a string and therefore can't be parsed at all
                match &tagged_value.value {
                    Value::String(reference) => {
                        let reference = reference.parse()?;
                        Ok(Some(reference))
                    }
                    other => Err(ReferenceError::NotAReference(other.clone())),
                }
            }
            _ => Ok(None),
        }
    }

    /// TODO
    fn lookup<'a>(
        &self,
        value: &'a Value,
        path: &ReferencePath,
    ) -> Result<&'a Value, ReferenceError> {
        match path.first_rest() {
            Some((first, rest)) => {
                // We need to go deeper. Value better be something we can drill
                // into
                match value {
                    Value::Null
                    | Value::Bool(_)
                    | Value::Number(_)
                    | Value::String(_) => todo!("return error"),
                    Value::Sequence(vec) => {
                        let index: usize = first.parse().expect("TODO");
                        // TODO can we dedupe this error? maybe just return
                        // none?
                        let inner = vec.get(index).ok_or_else(|| {
                            ReferenceError::NoResource(self.clone())
                        })?;
                        self.lookup(inner, &rest)
                    }
                    Value::Mapping(mapping) => {
                        let inner = mapping.get(first).ok_or_else(|| {
                            ReferenceError::NoResource(self.clone())
                        })?;
                        self.lookup(inner, &rest)
                    }
                    Value::Tagged(tagged_value) => {
                        // Remove the tag and try again
                        self.lookup(&tagged_value.value, path)
                    }
                }
            }
            // This is the end of the reference, we found our value
            None => Ok(value),
        }
    }
}

/// Serialization
impl From<Reference> for String {
    fn from(value: Reference) -> Self {
        value.to_string()
    }
}

impl FromStr for Reference {
    type Err = ReferenceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_reference(s)
    }
}

impl TryFrom<&str> for Reference {
    type Error = <Self as FromStr>::Err;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#", self.source)?;
        for component in &*self.path.0 {
            write!(f, "/{component}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ReferenceSource {
    Local,
}

impl Display for ReferenceSource {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            ReferenceSource::Local => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ReferencePath<'a>(Cow<'a, [String]>);

impl<'a> ReferencePath<'a> {
    fn first_rest(&'a self) -> Option<(&'a str, Self)> {
        match &*self.0 {
            [] => None,
            [first, rest @ ..] => {
                Some((first, ReferencePath(Cow::Borrowed(rest))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_yaml::from_str;

    /// Test loading valid references
    #[rstest]
    #[case::no_reference("3", "3")]
    #[case::simple_reference(
        r##"
        requests:
            login:
                username: "user"
                password: "pass"
            ref_login: !ref "#/requests/login"
        "##,
        r##"
        requests:
            login:
                username: "user"
                password: "pass"
            ref_login:
                username: "user"
                password: "pass"
        "##
    )]
    #[case::nested(
        // TODO this isn't nested??
        r##"
        requests:
            login:
                username: "user"
                password: "pass"
            details: !ref "#/requests/login"
        "##,
        r##"
        requests:
            login:
                username: "user"
                password: "pass"
            details:
                username: "user"
                password: "pass"
        "##
    )]
    fn test_successful_references(#[case] input: &str, #[case] expected: &str) {
        let mut input: Value = from_str(input).unwrap();
        let expected: Value = from_str(expected).unwrap();
        input.resolve_references().unwrap();
        assert_eq!(input, expected);
    }

    /// Test handling of invalid references
    #[rstest]
    #[case::invalid_reference(
        r##"ref_invalid: !ref "bad ref""##,
        ReferenceError::InvalidReference("bad ref".into())
    )]
    #[case::not_a_reference(
        "ref_invalid: !ref 3",
        ReferenceError::NotAReference(from_str("3").unwrap())
    )]
    #[case::no_resource(
        r##"
        requests:
        ref_invalid: !ref "#/requests/invalid"
        "##,
        ReferenceError::NoResource("#/requests/invalid".parse().unwrap()),
    )]
    #[case::circular_self(
        r##"ref_self: !ref "#/ref_self""##,
        ReferenceError::CircularReference
    )]
    #[case::circular_mutual(
        r##"
        ref1: !ref "#/ref2"
        ref2: !ref "#/ref1"
        "##,
        ReferenceError::CircularReference
    )]
    #[case::circular_parent(
        r##"
        root:
            inner: !ref "#/root"
        "##,
        ReferenceError::CircularReference
    )]
    fn test_errors(
        #[case] input: &str,
        #[case] expected_error: ReferenceError,
    ) {
        let mut input: Value = from_str(input).unwrap();
        let result = input.resolve_references();
        assert_eq!(result.unwrap_err(), expected_error);
    }
}
