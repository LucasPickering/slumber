//! Resolve $ref tags in YAML documents

use crate::collection::cereal::{LocatedError, yaml_parse_panic};
use saphyr::{AnnotatedMapping, MarkedYaml, Scalar, YamlData};
use slumber_util::NEW_ISSUE_LINK;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{self, Display},
    str::FromStr,
};
use thiserror::Error;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, preceded, repeat, separated_pair},
    error::EmptyError,
    token::take_while,
};

/// Mapping key denoting a reference
pub const REFERENCE_KEY: &str = "$ref";

type Result<T> = std::result::Result<T, LocatedError<ReferenceError>>;

/// Resolve `$ref` keys in a YAML document. `$ref` uses [the syntax from
/// OpenAPI](https://swagger.io/docs/specification/v3_0/using-ref/#ref-syntax).
pub trait ResolveReferences {
    /// Mutate this YAML value, replacing each reference with its resolved
    /// value. Return an error if any reference fails to resolve
    fn resolve_references(&mut self) -> Result<()>;
}

impl ResolveReferences for MarkedYaml<'_> {
    fn resolve_references(&mut self) -> Result<()> {
        Resolver::new().resolve_all(self)
    }
}

/// Error while parsing or resolving a reference
#[derive(Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum ReferenceError {
    /// YAML value is not a string and therefore can't be a reference
    #[error("Not a reference")]
    // Could potentially include the invalid value here, shortcutting for now
    NotAReference,
    /// Failed to parse a string to a reference
    #[error("Invalid reference: `{0}`")]
    InvalidReference(String),
    /// Reference parsed correctly but doesn't point to a resource
    #[error("Resource does not exist: `{0}`")]
    NoResource(Reference),
    #[error("Circular references")]
    CircularReference,
    #[error(
        "Expected reference `{reference}` to refer to a mapping, as the \
        referring parent is a mapping with multiple fields. The referenced \
        value must also be a mapping so it can be spread into the referring map"
    )]
    ExpectedMapping { reference: Reference },
    /// Step 2 of reference resolution (replaced references with values)
    /// encountered a reference that wasn't resolved in step 1
    #[error(
        "Reference `{0}` was not resolved. Please report this as a bug! {link}",
        link = NEW_ISSUE_LINK
    )]
    Unresolved(Reference),
}

/// Helper for resolving all references in a YAML document
///
/// Resolution happens in two stages:
/// - Resolve each reference and cache it here
/// - Make a second pass over the document and replace its reference with its
///   resolved value
///
/// This two-pass approach is needed because we can't modify the document while
/// iterating over it.
struct Resolver<'input> {
    /// References that have already been resolved
    resolved: HashMap<Reference, MarkedYaml<'input>>,
}

impl<'input> Resolver<'input> {
    fn new() -> Self {
        Self {
            resolved: HashMap::new(),
        }
    }

    fn resolve_all(mut self, value: &mut MarkedYaml<'input>) -> Result<()> {
        self.load_references(value, value)?;
        self.replace_references(value)?;
        Ok(())
    }

    /// Traverse the YAML value and for any reference, resolve its corresponding
    /// value and insert that value into the resolved map.
    fn load_references(
        &mut self,
        root: &MarkedYaml<'input>,
        value: &MarkedYaml<'input>,
    ) -> Result<()> {
        match &value.data {
            // Nothing to do on scalars
            YamlData::Value(_) => Ok(()),
            // Drill down into collections
            YamlData::Sequence(sequence) => {
                for value in sequence {
                    self.load_references(root, value)?;
                }
                Ok(())
            }
            YamlData::Mapping(mapping) => {
                for (key, value) in mapping {
                    if key.data.as_str() == Some(REFERENCE_KEY) {
                        // Key is $ref; value should be a reference
                        let reference = Reference::try_from_yaml(value)?;
                        let source = self.source(&reference, root);
                        let value =
                            reference.path.lookup(source).ok_or_else(|| {
                                LocatedError {
                                    error: ReferenceError::NoResource(
                                        reference.clone(),
                                    ),
                                    // Report the error location as the final
                                    // value we were able to successfully
                                    // traverse. Maybe this should be the
                                    // location of the reference instead, but
                                    // the value location is easier to get
                                    location: value.span.start,
                                }
                            })?;
                        self.resolved.insert(reference, value.clone());
                    } else {
                        self.load_references(root, value)?;
                    }
                }
                Ok(())
            }
            YamlData::Tagged(_, value) => self.load_references(root, value),
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
        }
    }

    /// Replace all references with computed values. Every `$ref` field in a
    /// mapping will be replaced by its resolved value. If the referencing
    /// object contains keys beyond `$ref`, the referenced value will be spread
    /// into the referencing mapping. This requires the referenced value to be
    /// a mapping as well.
    ///
    /// ```yaml
    /// refs:
    ///   mapping:
    ///     key: value
    ///   scalar: hello
    ///
    /// scalar:
    ///   $ref: #/scalar
    /// mapping:
    ///   $ref: #/mapping
    /// spread:
    ///   $ref: #/mapping
    ///   extra: 3
    /// ```
    ///
    /// maps to
    ///
    /// ```yaml
    /// scalar: hello
    /// mapping:
    ///     key: value
    /// spread:
    ///     key: value
    ///     extra: 3
    /// ```
    fn replace_references(&self, value: &mut MarkedYaml<'input>) -> Result<()> {
        match &mut value.data {
            // Nothing to do on scalars
            YamlData::Value(_) => Ok(()),
            // Drill down into collections
            YamlData::Sequence(sequence) => {
                for value in sequence {
                    self.replace_references(value)?;
                }
                Ok(())
            }
            YamlData::Mapping(mapping) => {
                match mapping.get(&MarkedYaml::value_from_str(REFERENCE_KEY)) {
                    Some(reference) if mapping.len() == 1 => {
                        // We have a mapping with just the key $ref. Replace the
                        // entire mapping with the referenced value
                        let reference = Reference::try_from_yaml(reference)?;
                        let resolved = self.get(&reference);
                        *value = resolved;
                        Ok(())
                    }

                    // We have a mapping with $ref as well as other keys. Spread
                    // the referenced value into the mapping
                    Some(_) => self.spread_mapping(mapping),

                    // No reference to resolve here, just handle values
                    None => {
                        for value in mapping.values_mut() {
                            self.replace_references(value)?;
                        }
                        Ok(())
                    }
                }
            }
            YamlData::Tagged(_, value) => self.replace_references(value),
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
        }
    }

    /// Given a mapping that contains a `$ref` key, spread the referenced
    /// mapping into the referencing mapping. If the reference doesn't point to
    /// a mapping, return an error. The mapping will be spread as if all fields
    /// from the referenced mapping were defined where `$ref` is in the original
    /// mapping. This means fields before the `$ref` will be overwritten by
    /// fields in the referenced mapping. Fields after the `$ref` will overwrite
    /// fields in the referenced mapping.
    ///
    /// ```yaml
    /// refs:
    ///   mapping:
    ///     a: 1
    ///     b: 2
    ///
    /// mapping:
    ///     a: 0
    ///     $ref: #/refs/mapping
    ///     b: 3
    /// ```
    ///
    /// maps to
    ///
    /// ```yaml
    /// mapping:
    ///   a: 1
    ///   b: 3
    /// ```
    fn spread_mapping(
        &self,
        mapping: &mut AnnotatedMapping<MarkedYaml<'input>>,
    ) -> Result<()> {
        // We have to map from a linked hashmap to a vec because:
        // - Granular control over the ordering of keys, to ensure the last
        //   occurrence of each key is the one that's kept
        // - LinkedHashMap's cursor doesn't support deletion
        // To prevent shifting, we'll copy into the vec incrementally
        let mut vec: Vec<_> = Vec::with_capacity(mapping.len());

        for (key, mut value) in mapping.drain() {
            if key.data.as_str() == Some(REFERENCE_KEY) {
                // Replace the reference with its fields
                let reference = Reference::try_from_yaml(&value)?;
                let resolved = self.get(&reference);

                let YamlData::Mapping(resolved_mapping) = resolved.data else {
                    return Err(LocatedError {
                        error: ReferenceError::ExpectedMapping { reference },
                        location: resolved.span.start,
                    });
                };
                vec.extend(resolved_mapping);
            } else {
                // Recursion!
                self.replace_references(&mut value)?;
                // Put the resolved value back in the map
                vec.push((key, value));
            }
        }

        *mapping = vec.into_iter().collect();
        Ok(())
    }

    /// Get the YAML document corresponding to a reference's source
    fn source<'a>(
        &self,
        reference: &Reference,
        local_value: &'a MarkedYaml<'input>,
    ) -> &'a saphyr::MarkedYaml<'input> {
        match &reference.source {
            ReferenceSource::Local => local_value,
        }
    }

    /// Get a referenced value. If the reference has not already been resolved,
    /// panic because that indicates a bug
    fn get(&self, reference: &Reference) -> MarkedYaml<'input> {
        // We have to clone because the value may be referenced more than once
        self.resolved
            .get(reference).unwrap_or_else(|| panic!(
                "Reference `{reference}` was not resolved. Please report this as a bug! {NEW_ISSUE_LINK}",
            ))
            .clone()
    }
}

/// A pointer to a YAML value. The reference points to a particular YAML
/// document (`source`) and a path to the value within that document (`path`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Reference {
    source: ReferenceSource,
    path: ReferencePath<'static>,
}

impl Reference {
    /// Attempt to parse a YAML value as a reference. This should be the value
    /// assigned to the `$ref` key, *not* the parent mapping. The value must be
    /// a string that parses as a valid URI.
    fn try_from_yaml(value: &MarkedYaml) -> Result<Self> {
        // We can hit two error cases:
        // - It's not a string and therefore can't be parsed
        // - It's a string but can't parse into a reference
        if let YamlData::Value(Scalar::String(reference)) = &value.data {
            let reference =
                reference.parse().map_err(|error| LocatedError {
                    error,
                    location: value.span.start,
                })?;
            Ok(reference)
        } else {
            Err(LocatedError {
                error: ReferenceError::NotAReference,
                location: value.span.start,
            })
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

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // A reference is just a URL with a customized base. Base options are:
        // - Empty: Local
        // - File path: Another file
        // - HTTP/HTTPS host: Remote file

        fn source(
            input: &mut &str,
        ) -> ModalResult<ReferenceSource, EmptyError> {
            alt((
                "".map(|_| ReferenceSource::Local),
                // More sources to come...
            ))
            .parse_next(input)
        }

        /// Parse path after `#`
        fn path(
            input: &mut &str,
        ) -> ModalResult<ReferencePath<'static>, EmptyError> {
            let segment = preceded('/', take_while(1.., |c| c != '/'));

            repeat(1.., segment)
                .fold(Vec::new, |mut acc, item: &str| {
                    acc.push(item.to_owned());
                    acc
                })
                .map(|segments| ReferencePath(Cow::Owned(segments)))
                .parse_next(input)
        }

        let (source, path) = separated_pair(source, '#', path)
            .parse(s)
            .map_err(|_| ReferenceError::InvalidReference(s.to_owned()))?;

        Ok(Self { source, path })
    }
}

impl Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            ReferenceSource::Local => Ok(()),
        }
    }
}

/// Everything to the right of the `#` in a reference, which refers to a
/// particular value in some YAML document. The lifetime allows this to be split
/// into `(first, rest)` without cloning during traversal.
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

    /// Traverse a YAML value according to this path, returning the value at the
    /// end of the rainbow. Return `None` if not found
    fn lookup<'input, 'value>(
        &self,
        value: &'value MarkedYaml<'input>,
    ) -> Option<&'value MarkedYaml<'input>> {
        if let Some((first, rest)) = self.first_rest() {
            // We need to go deeper. Value better be something we can drill into
            match &value.data {
                YamlData::Value(_) => None,
                YamlData::Sequence(sequence) => {
                    // Parse the segment as an int. If parsing fails, we can
                    // report this as a generic "no resource" error, as there
                    // isn't a traversable resource at the given path
                    let index: usize = first.parse().ok()?;
                    let inner = sequence.get(index)?;
                    rest.lookup(inner)
                }
                YamlData::Mapping(mapping) => {
                    let inner = mapping
                        // Clone is necessary to prevent lifetime fuckery.
                        // With &str, the lifetime of `first` gets promoted
                        // to the lifetime param on MarkedYaml, which is
                        // `'input`
                        .get(&MarkedYaml::scalar_from_string(
                            first.to_owned(),
                        ))?;
                    rest.lookup(inner)
                }
                YamlData::Tagged(_, value) => {
                    // Remove the tag and try again
                    self.lookup(value)
                }
                YamlData::Representation(_, _, _)
                | YamlData::BadValue
                | YamlData::Alias(_) => yaml_parse_panic(),
            }
        } else {
            // End of the line!!
            Some(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use saphyr::LoadableYamlNode;
    use slumber_util::{TempDir, assert_err, temp_dir};
    use std::fs;

    /// Test loading valid references
    #[rstest]
    #[case::no_reference("3", "3")]
    #[case::simple_reference(
        r##"
        requests:
            login:
                username: "user"
                password: "pass"
            ref_login:
                $ref: "#/requests/login"
        "##,
        r#"
        requests:
            login:
                username: "user"
                password: "pass"
            ref_login:
                username: "user"
                password: "pass"
        "#
    )]
    #[case::sequence(
        r##"
        base:
            seq:
                - a
                - b

        value:
            $ref: "#/base/seq/1"
        "##,
        r"
        base:
            seq:
                - a
                - b
        value: b
        "
    )]
    #[ignore = "Nested references not implemented yet"]
    #[case::nested(
        // Two levels of references
        r##"
        base:
            headers:
                Content-Type: application/json

        requests:
            login:
                $ref: "#/base/headers"
            details:
                $ref: "#/requests/login"
        "##,
        r"
        base:
            headers:
                Content-Type: application/json

        requests:
            login:
                headers:
                    Content-Type: application/json
            details:
                headers:
                    Content-Type: application/json
        "
    )]
    #[case::spread(
        r##"
        base:
            a: 1
            b: 1

        child:
            a: 0
            $ref: "#/base"
            b: 2
        "##,
        r"
        base:
            a: 1
            b: 1

        child:
            a: 1
            b: 2
        "
    )]
    fn test_references(#[case] input: &str, #[case] expected: &str) {
        let mut input = parse_yaml(input);
        let expected = parse_yaml(expected);
        input.resolve_references().unwrap();
        assert_eq!(input, expected);
    }

    /// Load a reference to another file
    #[rstest]
    #[ignore = "File references not implemented yet"]
    fn test_reference_file(temp_dir: TempDir) {
        // Create a file with its own references
        let file_name = "other.yml";
        let path = temp_dir.join(file_name);
        fs::write(
            &path,
            r##"
requests:
    r1:
        url: test
    r2:
        url:
            $ref: "#/r1/url"
"##,
        )
        .unwrap();

        // Test both absolute and relative paths
        let input = format!(
            r#"
absolute:
    $ref: "{path_abs}#/r2/url"
relative:
    $ref: "{path_rel}#/r2/url"
"#,
            path_abs = path.display(),
            path_rel = file_name,
        );
        let mut input = parse_yaml(&input);
        let expected = parse_yaml(
            r"
absolute: test
relative: test
",
        );
        input.resolve_references().unwrap();
        assert_eq!(input, expected);
    }

    /// Cross-file reference cycles should be detected and throw an error
    #[rstest]
    #[ignore = "File references not implemented yet"]
    fn test_reference_file_cycle(temp_dir: TempDir) {
        let file1 = "file1.yml";
        let file2 = "file2.yml";
        fs::write(
            temp_dir.join(file1),
            r#"
data:
    $ref: "file2.yml#/data"
"#,
        )
        .unwrap();
        fs::write(
            temp_dir.join(file2),
            r#"
data:
    $ref: "file1.yml#/data"
"#,
        )
        .unwrap();

        let yaml = fs::read_to_string(temp_dir.join(file1)).unwrap();
        let mut input = parse_yaml(&yaml);
        assert_err!(
            input.resolve_references(),
            "Circular references: `file2.yml#/data` -> `file1.yml#/data` -> `file2.yml#/data`"
        );
    }

    /// Test handling of invalid references
    #[rstest]
    #[case::invalid_reference(
        r#"ref_invalid:
            $ref: "bad ref""#,
        "Invalid reference: `bad ref`"
    )]
    #[case::not_a_reference(
        "ref_invalid:
            $ref: 3",
        "Not a reference"
    )]
    #[case::too_many_segments(
        r##"
        value: 3
        ref_invalid:
            $ref: "#/value/x"
        "##,
        "Resource does not exist: `#/value/x`"
    )]
    #[case::sequence_index_out_of_bounds(
        r##"
        seq: [a, b]
        ref_invalid:
            $ref: "#/seq/2"
        "##,
        "Resource does not exist: `#/seq/2`"
    )]
    #[case::sequence_string_segment(
        r##"
        seq: [a, b]
        ref_invalid:
            $ref: "#/seq/w"
        "##,
        "Resource does not exist: `#/seq/w`"
    )]
    #[case::mapping_unknown_key(
        r##"
        map:
            a: 1
            b: 2
        ref_invalid:
            $ref: "#/map/c"
        "##,
        "Resource does not exist: `#/map/c`"
    )]
    #[ignore = "Nested references not implemented yet"]
    #[case::circular_self(
        r##"ref_self:
            $ref: "#/ref_self""##,
        "Circular references: `#/ref_self` -> `#/ref_self`"
    )]
    #[ignore = "Nested references not implemented yet"]
    #[case::circular_mutual(
        r##"
        ref1:
            $ref: "#/ref2"
        ref2:
            $ref: "#/ref1"
        "##,
        "Circular references: `#/ref1` -> `#/ref2` -> `#/ref1`"
    )]
    #[ignore = "Nested references not implemented yet"]
    #[case::circular_parent(
        r##"
        root:
            inner:
                $ref: "#/root"
        "##,
        "Circular references: `#/root` -> `#/root`"
    )]
    #[ignore = "File references not implemented yet"]
    #[case::io(
        r#"
        root:
            $ref: "./other.yml#/root"
        "#,
        "File not found: ./other.yml"
    )]
    fn test_errors(#[case] input: &str, #[case] expected_error: &str) {
        let mut input = parse_yaml(input);
        let result = input.resolve_references();
        assert_err!(result, expected_error);
    }

    fn parse_yaml(yaml: &str) -> MarkedYaml {
        let mut documents = MarkedYaml::load_from_str(yaml).unwrap();
        documents.pop().unwrap()
    }
}
