//! Resolve $ref tags in YAML documents

use crate::collection::cereal::{LocatedError, yaml_parse_panic};
use derive_more::From;
use indexmap::IndexMap;
use itertools::Itertools;
use saphyr::{AnnotatedMapping, MarkedYaml, Marker, Scalar, YamlData};
use slumber_util::NEW_ISSUE_LINK;
use std::{
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
        // The inability to both traverse and modify the document at the same
        // time means we have to transform the document in a few discrete steps:
        // - Collect all references in the YAML doc
        // - Build a dependency graph of all the references
        // - Sort the reference graph topologically so we can resolve them in
        //   order without missing any references. This will also identify
        //   cycles
        // - Replace each reference with its resolved value
        // We only iterate over the entire value once, in the first step.
        // Everything after that operates just on the list of references. So
        // steps 2+ scale with the number of references, not the size of the
        // YAML.
        ReferenceLocations::find(self)?
            // Convert the reference list into a dependency graph
            .build_graph()
            // Topologically sort it
            .sort()?
            // Replace each reference
            .replace(self)
    }
}

/// A collection all references in the YAML document, each one key by the
/// location of its *usage* in the document. Locations are unique but references
/// are not, as the same reference can be used multiple times.
struct ReferenceLocations<'input>(Vec<(YamlPath<'input>, Reference)>);

impl<'input> ReferenceLocations<'input> {
    /// Find all references in the document, keyed by their location. Fails if
    /// any references fail to parse. References are *not* resolved, so this
    /// will *not* fail for dangling references.
    fn find(value: &MarkedYaml<'input>) -> Result<Self> {
        let mut locations = Self(Vec::new());
        locations.find_recursive(YamlPath::new(), value)?;
        Ok(locations)
    }

    /// Recursive helper for [Self::find]
    fn find_recursive(
        &mut self,
        path: YamlPath<'input>,
        value: &MarkedYaml<'input>,
    ) -> Result<()> {
        match &value.data {
            // Nothing to do on scalars
            YamlData::Value(_) => Ok(()),
            // Drill down into collections
            YamlData::Sequence(sequence) => {
                for (index, value) in sequence.iter().enumerate() {
                    self.find_recursive(
                        path.cons(index, value.span.start),
                        value,
                    )?;
                }
                Ok(())
            }
            // Look for a mapping with the $ref key
            YamlData::Mapping(mapping) => {
                // It's possible for a mapping to have other fields in addition
                // to $ref
                for (key, value) in mapping {
                    if key.data.as_str() == Some(REFERENCE_KEY) {
                        // Key is $ref; value should be a reference
                        let reference = Reference::try_from_yaml(value)?;
                        self.0.push((path.clone(), reference));
                    } else {
                        self.find_recursive(
                            path.cons(key.clone(), value.span.start),
                            value,
                        )?;
                    }
                }
                Ok(())
            }
            // Ignore the tag and drill into the value
            YamlData::Tagged(_, value) => self.find_recursive(path, value),
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
        }
    }

    /// Build a dependency graph from the set of all references. Each reference
    /// is mapped to the list of references that must be resolved ahead of it.
    fn build_graph(self) -> UnsortedGraph<'input> {
        // We have a list of every reference and where it is, which we can map
        // into a graph without having to look back at the document
        let mut graph: HashMap<Reference, ReferenceMetadata> = HashMap::new();
        for (location, reference) in &self.0 {
            graph
                .entry(reference.clone())
                // If we've already computed dependencies for this reference,
                // we can skip that and just add this location
                .and_modify(|metadata| {
                    metadata.locations.push(location.clone());
                })
                // First time seeing this reference: compute its dependencies.
                // This is O(n^2). There's probably an O(n) solution to
                // incrementally update each reference's dependencies but I'm
                // being lazy.
                .or_insert_with(|| {
                    let dependencies = self
                        .0
                        .iter()
                        .filter_map(|(path, other_reference)| {
                            // `reference` points to a value somewhere. Check if
                            // there are any other references at, above or below
                            // the path that `reference` points to. This would
                            // indicate that `other_reference` must be resolved
                            // before `reference`
                            if reference.depends_on(path) {
                                Some((other_reference.clone(), path.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();
                    ReferenceMetadata {
                        locations: vec![location.clone()],
                        dependencies,
                    }
                });
        }
        UnsortedGraph(graph)
    }
}

/// Metadata about a particular reference in a YAML document. This is all the
/// information we gather about a reference while traversing the YAML for all
/// references.
#[derive(Debug)]
struct ReferenceMetadata<'input> {
    /// Every location where this reference is used
    locations: Vec<YamlPath<'input>>,
    /// All references that must be resolved before this one. For each
    /// reference we track the particular location of that reference on
    /// which we're dependent. If we're dependent on the reference multiple
    /// times, only one location will appear. The location is tracked just
    /// so we can give a useful source location in the case of a cycle
    /// error.
    dependencies: HashMap<Reference, YamlPath<'input>>,
}

/// A graph of all the references in a YAML document, in no particular order.
/// Once [sorted](Self::sort), this will define a consistent resolution order
/// for the references.
struct UnsortedGraph<'input>(HashMap<Reference, ReferenceMetadata<'input>>);

impl<'input> UnsortedGraph<'input> {
    /// Step 2: Sort the graph topologically, such that every reference only has
    /// dependencies on the references before it in the map. This enables us
    /// to resolve+replace references in order without worrying about unresolved
    /// nested references.
    fn sort(mut self) -> Result<SortedGraph<'input>> {
        // https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm
        let mut sorted = IndexMap::new();
        // Working set of all references that have no dependencies
        let mut independent: Vec<(Reference, ReferenceMetadata)> = self
            .0
            .extract_if(|_, metadata| metadata.dependencies.is_empty())
            .collect();
        // All remaining references that have unsorted dependencies
        let mut dependents = self.0;

        while let Some((reference, metadata)) = independent.pop() {
            // Sanity check
            assert!(
                metadata.dependencies.is_empty(),
                "Reference still has dependencies: {:?}",
                metadata.dependencies
            );

            // Drop this dependency from everyone that needed it. This is O(n^2)
            // which could be fixed by caching the dependency map, but the
            // cardinalities are generally double digits so it's not worth.
            //
            // Any reference that was dependent on just the reference is now
            // independent
            let newly_independent = dependents.extract_if(|_, metadata| {
                metadata.dependencies.remove(&reference);
                metadata.dependencies.is_empty()
            });
            independent.extend(newly_independent);

            // See it. Say it. Sorted.
            sorted.insert(reference, metadata);
        }

        // If there's anything left in the dependent list, that means we have
        // a cycle
        if let Some(dependent) = dependents.values().next() {
            // We need a source location to include with the error. The
            // dependent is guaranteed to have at least one dependency, and its
            // only remaining dependencies will be part of the cycle(s). So we
            // can grab the location of any dependency and be sure it will be
            // part of a cycle. The beautiful thing about a cycle is we can
            // point to any node in it and the user should be able to find the
            // whole dang thing.
            let location =
                dependent.dependencies.values().next().unwrap().location;
            Err(LocatedError {
                error: ReferenceError::CircularReference {
                    // Sort for predictable ordering
                    references: dependents.into_keys().sorted().collect(),
                },
                location,
            })
        } else {
            Ok(SortedGraph(sorted))
        }
    }
}

/// Same as [UnsortedGraph], but the references have been sorted topologically
/// so that each reference only has dependencies on the references before it
/// in the map.
struct SortedGraph<'input>(IndexMap<Reference, ReferenceMetadata<'input>>);

impl<'input> SortedGraph<'input> {
    /// Step 3: Replace all references with computed values. Every `$ref` field
    /// in a mapping will be replaced by its resolved value. If the
    /// referencing object contains keys beyond `$ref`, the referenced value
    /// will be spread into the referencing mapping. This requires the
    /// referenced value to be a mapping as well.
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
    fn replace(self, root: &mut MarkedYaml<'input>) -> Result<()> {
        // For each reference in the document, resolve it any replace all
        // instances of the reference with the resolved value. Because the
        // references are in topological order, we know each one will only
        // have dependencies on its predecessors. Because we do each replacement
        // inline, the dependencies will automatically be resolved in their
        // dependents.
        for (reference, metadata) in self.0 {
            // Clone necessary to release the ref on `root`, allowing us to
            // take another mutable reference to it
            let resolved = reference.resolve(root)?.clone();
            for path in metadata.locations {
                let value = path.get(root);
                // The collection process for references ensures that each path
                // points to a mapping containing a `$ref`
                let YamlData::Mapping(mapping) = &mut value.data else {
                    panic!(
                        "Expected path {path:?} to point to a mapping, \
                        but found {value:?}"
                    )
                };
                if mapping.len() == 1 {
                    *value = resolved.clone();
                } else {
                    // We have a mapping with $ref as well as other keys. Spread
                    // the referenced value into the mapping
                    // The value to spread must be a mapping!
                    let YamlData::Mapping(to_spread) = resolved.clone().data
                    else {
                        return Err(LocatedError {
                            error: ReferenceError::ExpectedMapping {
                                reference,
                                parent: value.span.start,
                            },
                            location: resolved.span.start,
                        });
                    };
                    spread_mapping(mapping, to_spread);
                }
            }
        }
        Ok(())
    }
}

/// Traverse a YAML value according a reference path, returning the value at the
/// end of the rainbow. If not found, return the source location of the deepest
/// value we successfully traversed, which is also the location of
/// where the path went cold.
fn reference_lookup<'input, 'value>(
    value: &'value MarkedYaml<'input>,
    path: &[String],
) -> std::result::Result<&'value MarkedYaml<'input>, Marker> {
    if let [first, rest @ ..] = path {
        let location = value.span.start;
        // We need to go deeper. Value better be something we can drill into
        match &value.data {
            YamlData::Value(_) => Err(location),
            YamlData::Sequence(sequence) => {
                // Parse the segment as an int. If parsing fails, we can
                // report this as a generic "no resource" error, as there
                // isn't a traversable resource at the given path
                let index: usize = first.parse().or(Err(location))?;
                let inner = sequence.get(index).ok_or(location)?;
                reference_lookup(inner, rest)
            }
            YamlData::Mapping(mapping) => {
                let inner = mapping
                    // Clone is necessary to prevent lifetime fuckery.
                    // With &str, the lifetime of `first` gets promoted
                    // to the lifetime param on MarkedYaml, which is
                    // `'input`
                    .get(&MarkedYaml::scalar_from_string(first.to_owned()))
                    .ok_or(location)?;
                reference_lookup(inner, rest)
            }
            YamlData::Tagged(_, value) => {
                // Remove the tag and try again
                reference_lookup(value, rest)
            }
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
        }
    } else {
        // End of the line!!
        Ok(value)
    }
}

/// Replace the `$ref` key in a mapping with the reference's resolved value.
/// The resolution is performed by the caller. The caller must enforce that the
/// reference pointed to a mapping, as spreading is impossible otherwise. The
/// mapping will be spread as if all fields from the referenced mapping were
/// defined where `$ref` is in the original mapping. This means fields before
/// the `$ref` will be overwritten by fields in the referenced mapping. Fields
/// after the `$ref` will overwrite fields in the referenced mapping.
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
/// spreads to
///
/// ```yaml
/// mapping:
///   a: 1
///   b: 3
/// ```
fn spread_mapping<'input>(
    mapping: &mut AnnotatedMapping<MarkedYaml<'input>>,
    to_spread: AnnotatedMapping<MarkedYaml<'input>>,
) {
    // We have to map from a linked hashmap to a vec because:
    // - Granular control over the ordering of keys, to ensure the last
    //   occurrence of each key is the one that's kept
    // - LinkedHashMap's cursor doesn't support deletion
    // To prevent shifting, we'll copy into the vec incrementally
    let mut vec: Vec<_> = Vec::with_capacity(mapping.len());

    // We know that the mapping only needs to be spread once because there
    // can't be more than one $ref field in a mapping (keys are unique). But
    // the borrow checker doesn't know that. The option convinces the borrow
    // checker that we won't use `to_spread` more than once
    let mut to_spread = Some(to_spread);
    for (key, value) in mapping.drain() {
        if key.data.as_str() == Some(REFERENCE_KEY) {
            // Replace the reference with its fields
            vec.extend(to_spread.take().unwrap());
        } else {
            // Put the value back in the map
            vec.push((key, value));
        }
    }

    *mapping = vec.into_iter().collect();
}

/// A pointer to a YAML value. The reference points to a particular YAML
/// document (`source`) and a path to the value within that document (`path`).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Reference {
    source: ReferenceSource,
    path: Vec<String>,
}

impl Reference {
    /// Find the value referred to by this reference
    fn resolve<'value, 'input>(
        &self,
        root: &'value MarkedYaml<'input>,
    ) -> Result<&'value MarkedYaml<'input>> {
        reference_lookup(root, &self.path).map_err(|location| LocatedError {
            error: ReferenceError::NoResource(self.clone()),
            // Error location points to the deepest value we were able
            // to traverse. Using the location of
            // the reference may be more intuitive,
            // but this is easier to get and it points the user
            // to where the reference ceased to be valid
            location,
        })
    }

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

    /// Does this reference depend on a reference at the given location?
    /// Reference A depends on Reference B if:
    /// - A refers directly to where B is defined; A is an alias of B and will
    ///   to resolve to whatever B resolves to
    /// - A refers to a parent of B; B must be resolved in order to get a
    ///   complete value for A
    /// - A refers to a child of B; B must be resolved in order to completely
    ///   follow the path of A
    ///
    /// We don't actually need to know *where* B points. If we can establish
    /// that there's a dependency, then as long as B is resolved and replaced
    /// first, A can be resolved correctly.
    fn depends_on(&self, location: &YamlPath) -> bool {
        self.path
            .iter()
            .zip(location.segments.iter())
            .all(|(ref_part, location_part)| location_part == ref_part.as_str())
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
        fn path(input: &mut &str) -> ModalResult<Vec<String>, EmptyError> {
            let segment = preceded('/', take_while(1.., |c| c != '/'));

            repeat(1.., segment)
                .fold(Vec::new, |mut acc, item: &str| {
                    acc.push(item.to_owned());
                    acc
                })
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
        for component in &*self.path {
            write!(f, "/{component}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

/// A path to a sub-value within some YAML value. This is a series of path
/// segments that define how a YAML value should be traversed in order to access
/// some inner value.
///
/// This is very similar to [Reference], but is built programatically rather
/// than being parsed from user-provided input. This allows it to be more
/// flexible and use full YAML values as keys.
///
/// These are used to track locations of `$ref` objects. The path points to the
/// **parent mapping** of the reference, i.e. the mapping *containing* the
/// `$ref` key.
#[derive(Clone, Debug)]
struct YamlPath<'input> {
    segments: Vec<YamlPathSegment<'input>>,
    /// Source location of the *value* associated with this path. Used for
    /// error messages
    location: Marker,
}

impl<'input> YamlPath<'input> {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
            location: Marker::default(),
        }
    }

    /// Add a segment to this path, returning a new path
    fn cons(
        &self,
        segment: impl Into<YamlPathSegment<'input>>,
        location: Marker,
    ) -> Self {
        let mut segments = self.segments.clone();
        segments.push(segment.into());
        Self { segments, location }
    }

    /// Extract a sub-value from a root YAML value by traversing according to
    /// this path
    fn get<'value>(
        &self,
        root: &'value mut MarkedYaml<'input>,
    ) -> &'value mut MarkedYaml<'input> {
        let mut value: &'value mut MarkedYaml<'input> = root;
        for segment in &self.segments {
            match (segment, &mut value.data) {
                (
                    YamlPathSegment::Sequence(index),
                    YamlData::Sequence(sequence),
                ) => {
                    value = &mut sequence[*index];
                }
                (YamlPathSegment::Mapping(key), YamlData::Mapping(mapping)) => {
                    value = &mut mapping[key];
                }
                // Path should match the value from which it was constructed
                (_, value) => panic!(
                    "Expected a sequence or mapping to match path {self:?} \
                    at segment {segment:?}, but found {value:?}"
                ),
            }
        }
        value
    }
}

/// One segment in a [YamlPath]. This specifies a particular child in a
/// collection
#[derive(Clone, Debug, Eq, From, Hash, PartialEq)]
enum YamlPathSegment<'input> {
    /// Get a sequence element by index
    Sequence(usize),
    /// Get a mapping element by key
    Mapping(MarkedYaml<'input>),
}

#[cfg(test)]
impl From<&'static str> for YamlPathSegment<'static> {
    fn from(value: &'static str) -> Self {
        Self::Mapping(MarkedYaml::value_from_str(value))
    }
}

/// Compare a path segment to a reference segment. Since references
/// are parsed from strings, there's no way to discern an int from a string (`0`
/// vs `"0"`), so this treats the two as equal.
impl PartialEq<str> for YamlPathSegment<'_> {
    fn eq(&self, other: &str) -> bool {
        match self {
            Self::Sequence(index) => Ok(*index) == other.parse::<usize>(),
            Self::Mapping(key) => key.data.as_str() == Some(other),
        }
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
    /// There's a cycle in the reference graph somewhere. Unfortunately the
    /// topological sort algorithm we use doesn't tell us how many disjoint
    /// cycles there are or which references belong to which cycles, but just
    /// listing *all* the offending references should be helpful enough
    #[error(
        "References contain one or more cycles: {}",
        references.iter().format(", "),
    )]
    CircularReference { references: Vec<Reference> },
    #[error(
        "Expected reference `{reference}` to refer to a mapping, as the \
        referring parent at TODO:{}:{} is a mapping with multiple fields. \
        The referenced value must also be a mapping so it can be spread into \
        the referring map",
        parent.line(),parent.col(),
    )]
    ExpectedMapping {
        reference: Reference,
        parent: Marker,
    },
    /// Step 2 of reference resolution (replaced references with values)
    /// encountered a reference that wasn't resolved in step 1
    #[error(
        "Reference `{0}` was not resolved. Please report this as a bug! {link}",
        link = NEW_ISSUE_LINK
    )]
    Unresolved(Reference),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
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
    #[case::nested(
        // Referred value contains another reference that must be resolved
        r##"
        base:
            url: test

        requests:
            details:
                $ref: "#/requests/login"
            login:
                $ref: "#/base"
        "##,
        r"
        base:
            url: test

        requests:
            details:
                url: test
            login:
                url: test
        "
    )]
    #[case::nested_through(
        // Referred value contains a reference that gets traversed through
        r##"
        base:
            headers:
                Content-Type: application/json

        requests:
            details:
                headers:
                    $ref: "#/requests/login/headers"
            login:
                $ref: "#/base"
        "##,
        r"
        base:
            headers:
                Content-Type: application/json

        requests:
            details:
                headers:
                    Content-Type: application/json
            login:
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
            "References contain one or more cycles: \
            file2.yml#/data, file1.yml#/data, file2.yml#/data"
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
    #[case::spread_scalar(
        r##"
        map:
            a: 1
            b: 2
        ref_invalid:
            $ref: "#/map/a"
            b: 1
        "##,
        "Expected reference `#/map/a` to refer to a mapping"
    )]
    #[case::cycle_self(
        r##"ref_self:
            $ref: "#/ref_self""##,
        "References contain one or more cycles: #/ref_self"
    )]
    #[case::cycle_mutual(
        r##"
        ref1:
            $ref: "#/ref2"
        ref2:
            $ref: "#/ref1"
        "##,
        "References contain one or more cycles: #/ref1, #/ref2"
    )]
    #[case::cycle_parent(
        r##"
        root:
            inner:
                $ref: "#/root"
        "##,
        "References contain one or more cycles: #/root"
    )]
    #[case::cycle_multiple(
        // Multiple independent cycles
        r##"
        a: {"$ref": "#/b"}
        b: {"$ref": "#/a"}
        c: {"$ref": "#/d"}
        d: {"$ref": "#/c"}
        e: {"$ref": "#/f"}
        f: 1
        "##,
        // References outside the cycles are NOT included in the error message
        "References contain one or more cycles: #/a, #/b, #/c, #/d"
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

    /// Test [Reference::depends_on]
    #[rstest]
    #[case::ref_below("#/a", vec!["a", "b"], true)]
    #[case::ref_at("#/a/b", vec!["a", "b"], true)]
    #[case::ref_above("#/a/b/c", vec!["a", "b"], true)]
    #[case::disjoint("#/a/c", vec!["a", "b"], false)]
    fn test_depends_on(
        #[case] reference: Reference,
        #[case] segments: Vec<&'static str>,
        #[case] is_child: bool,
    ) {
        let path = YamlPath {
            segments: segments.into_iter().map(YamlPathSegment::from).collect(),
            location: Marker::default(),
        };
        assert_eq!(reference.depends_on(&path), is_child);
    }

    fn parse_yaml(yaml: &str) -> MarkedYaml {
        let mut documents = MarkedYaml::load_from_str(yaml).unwrap();
        documents.pop().unwrap()
    }
}
