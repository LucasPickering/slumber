//! Resolve $ref tags in YAML documents

use crate::{
    NEW_ISSUE_LINK, paths,
    yaml::{
        LocatedError, SourceId, SourceIdLocation, SourceLocation, SourceMap,
        SourcedYaml, yaml_parse_panic,
    },
};
use derive_more::From;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use saphyr::{AnnotatedMapping, Scalar, YamlData};
use std::{
    collections::HashMap,
    fmt::{self, Display},
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, preceded, repeat, separated_pair},
    error::EmptyError,
    token::{take_until, take_while},
};

/// Mapping key denoting a reference
pub const REFERENCE_KEY: &str = "$ref";

type Result<T> = std::result::Result<T, LocatedError<ReferenceError>>;

impl SourcedYaml<'_> {
    pub(super) fn resolve_references(
        self,
        source_map: &mut SourceMap,
    ) -> Result<Self> {
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
        ReferenceLocations::scan(self, source_map)?
            // Convert the reference list into a dependency graph
            .build_graph()
            // Topologically sort it
            .sort()?
            // Replace each reference
            .replace(source_map)
    }
}

/// A collection all references in the YAML document, each one key by the
/// location of its *usage* in the document. Locations are unique but references
/// are not, as the same reference can be used multiple times.
#[derive(Debug)]
struct ReferenceLocations<'input> {
    /// All found references
    references: Vec<(YamlPath<'input>, SourceReference)>,
    document_map: DocumentMap<'input>,
}

impl<'input> ReferenceLocations<'input> {
    /// Find all references in the document, keyed by their location. Fails if
    /// any references fail to parse. References are *not* resolved, so this
    /// will *not* fail for dangling references.
    fn scan(
        value: SourcedYaml<'input>,
        source_map: &mut SourceMap,
    ) -> Result<Self> {
        let mut scanner = ReferenceScanner {
            references: Vec::new(),
            source_map,
            unscanned_sources: IndexSet::new(),
        };

        // We can't have encountered more than one YAML document yet, which
        // means the source map can't have more than one entry. It's possible
        // for it to be empty though, if the YAML was loaded from memory.
        let source_map = &scanner.source_map;
        assert!(source_map.sources.len() <= 1);
        let root_source_id = if source_map.sources.is_empty() {
            SourceId::Memory
        } else {
            SourceId::File(0)
        };

        // Document map caches all the YAML documents that we've loaded
        let mut document_map = DocumentMap {
            root: value,
            root_source_id,
            additional: HashMap::new(),
        };

        // Start by scanning the root value
        // File imports will be relative to the root file. If the root value is
        // in memory instead of a file, we'll just pass None and relative file
        // imports are disallowed
        let root_path = source_map
            .get_path(root_source_id)
            .and_then(|path| path.parent())
            .map(Path::to_owned);
        scanner.scan_source(
            root_path.as_deref(),
            YamlPath::new(root_source_id),
            &document_map.root,
        )?;

        // Scan any additional sources encountered until we run out
        while let Some(source_id) = scanner.unscanned_sources.pop() {
            // Each source should've been added to the source map when it was
            // queued
            let file_path = scanner
                .source_map
                .get_path(source_id)
                // Clone needed to detach lifetime from source map
                .map(Path::to_owned)
                .unwrap_or_else(|| {
                    panic!("Source map missing source {source_id:?}")
                });

            // If this is the root, use the given value. Otherwise load the new
            // value from the file
            let value = match source_id {
                SourceId::File(_) => SourcedYaml::load(&file_path, source_id)
                    .map_err(|error| LocatedError {
                    error: ReferenceError::Nested(Box::new(error.error)),
                    location: error.location,
                })?,
                SourceId::Memory => {
                    // It shouldn't be possible for a Memory source to end up
                    // in the map
                    panic!("In-memory source cannot be referenced")
                }
            };

            // Scan this YAML for references
            scanner.scan_source(
                file_path.parent(),
                YamlPath::new(source_id),
                &value,
            )?;

            // Store the scanned value so we can use it for resolution later
            document_map.additional.insert(source_id, value);
        }

        Ok(Self {
            references: scanner.references,
            document_map,
        })
    }

    /// Build a dependency graph from the set of all references. Each reference
    /// is mapped to the list of references that must be resolved ahead of it.
    fn build_graph(self) -> UnsortedGraph<'input> {
        // We have a list of every reference and where it is, which we can map
        // into a dependency graph without having to look back at the document
        let mut graph: HashMap<SourceReference, ReferenceMetadata> =
            HashMap::new();

        for (path, reference) in &self.references {
            // For each reference, compare it against every other reference to
            // look for a dependency
            graph
                .entry(reference.clone())
                // If we've already computed dependencies for this reference,
                // we can skip that and just add this location
                .and_modify(|metadata| {
                    metadata.locations.push(path.clone());
                })
                // First time seeing this reference: compute its dependencies.
                // This is O(n^2). There's probably an O(n) solution to
                // incrementally update each reference's dependencies but I'm
                // being lazy.
                .or_insert_with(|| {
                    let dependencies = self
                        .references
                        .iter()
                        .filter_map(|(other_path, other_reference)| {
                            // `reference` points to a value somewhere. Check if
                            // `other_path` points at, above or below the path
                            // that `reference` points to. This would indicate
                            // that `other_reference` must be resolved before
                            // `reference`
                            if reference.depends_on(other_path) {
                                Some((
                                    other_reference.clone(),
                                    other_path.clone(),
                                ))
                            } else {
                                None
                            }
                        })
                        .collect();
                    ReferenceMetadata {
                        locations: vec![path.clone()],
                        dependencies,
                    }
                });
        }
        UnsortedGraph {
            references: graph,
            document_map: self.document_map,
        }
    }
}

/// State while scanning the YAML document tree for all references
#[derive(Debug)]
struct ReferenceScanner<'input, 'src> {
    /// All found references
    references: Vec<(YamlPath<'input>, SourceReference)>,
    source_map: &'src mut SourceMap,
    /// Queue of sources that have been encountered but not yet scanned.
    /// Paths are absolute to ensure there are no aliases.
    unscanned_sources: IndexSet<SourceId>,
}

impl<'input> ReferenceScanner<'input, '_> {
    /// Find all the references in a particular YAML source. References will be
    /// added to `self.references`. If any new sources are encountered along the
    /// way, they'll be added to `self.unscanned_sources`.
    ///
    /// ## Params
    ///
    /// - `reference_dir`: Directory that relative path imports will be resolved
    ///   from. This should be the directory containing the scanned file. `None`
    ///   for in-memory YAML, in which case file references are disallowed
    /// - `path`: Path to the YAML value being scanned. This gets built up as we
    ///   get further down the document
    /// - `value`: YAML value being scanned
    fn scan_source(
        &mut self,
        reference_dir: Option<&Path>,
        path: YamlPath<'input>,
        value: &SourcedYaml<'input>,
    ) -> Result<()> {
        match &value.data {
            // Nothing to do on scalars
            YamlData::Value(_) => Ok(()),
            // Drill down into collections
            YamlData::Sequence(sequence) => {
                for (index, value) in sequence.iter().enumerate() {
                    self.scan_source(
                        reference_dir,
                        path.cons(index, value.location),
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
                        self.add_reference(
                            reference_dir,
                            path.clone(),
                            reference,
                        );
                    } else {
                        self.scan_source(
                            reference_dir,
                            path.cons(key.clone(), value.location),
                            value,
                        )?;
                    }
                }
                Ok(())
            }
            // Ignore the tag and drill into the value
            YamlData::Tagged(_, value) => {
                self.scan_source(reference_dir, path, value)
            }
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
        }
    }

    /// Add a found reference to the collection
    ///
    /// ## Params
    ///
    /// - `reference_dir`: Root directory for relative file paths. `None` for
    ///   in-memory YAML, in which case file references are disallowed
    /// - `path`: YAML path to the reference (within the referencing document)
    /// - `reference`: Parsed reference to add
    fn add_reference(
        &mut self,
        reference_dir: Option<&Path>,
        path: YamlPath<'input>,
        reference: Reference,
    ) {
        // Check if the reference source is new. If so, add it to the queue of
        // sources to scan
        let source_id = match &reference.source {
            // Same file - the referenced source is the same as the referencing
            // one
            ReferenceSource::Local => path.source_id,
            ReferenceSource::File(path) => {
                // When resolving in-memory YAML, we can't do a file import
                // because we don't know where the import should be relative to.
                // We *could* allow absolute paths here still, but there isn't
                // a real use case for that so it's not worth the logic.
                //
                // The panic is ok here because in-memory YAML is only possible
                // from tests. THis shouldn't be reachable in app execution.
                let reference_dir = reference_dir.unwrap_or_else(|| {
                    panic!("File references disallowed from in-memory YAML")
                });

                // Get an absolute path to the referenced file, relative
                // to the given reference dir (which will be the parent of the
                // referencing file)
                let path = paths::normalize_path(reference_dir, path);

                if let Some(source_id) = self.source_map.get_source_id(&path) {
                    source_id
                } else {
                    // If this source hasn't been seen before, add it to the
                    // queue
                    let source_id = self.source_map.add_source(path.clone());
                    self.unscanned_sources.insert(source_id);
                    source_id
                }
            }
        };

        // Attach the source ID of the REFERENCED doc so we can easily track
        // what document it refers to later
        self.references.push((
            path,
            SourceReference {
                reference,
                source_id,
            },
        ));
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
    dependencies: HashMap<SourceReference, YamlPath<'input>>,
}

/// A graph of all the references in a YAML document, in no particular order.
/// Once [sorted](Self::sort), this will define a consistent resolution order
/// for the references.
#[derive(Debug)]
struct UnsortedGraph<'input> {
    references: HashMap<SourceReference, ReferenceMetadata<'input>>,
    document_map: DocumentMap<'input>,
}

impl<'input> UnsortedGraph<'input> {
    /// Step 2: Sort the graph topologically, such that every reference only has
    /// dependencies on the references before it in the map. This enables us
    /// to resolve+replace references in order without worrying about unresolved
    /// nested references.
    fn sort(mut self) -> Result<SortedGraph<'input>> {
        // https://en.wikipedia.org/wiki/Topological_sorting#Kahn's_algorithm
        let mut sorted = IndexMap::new();
        // Working set of all references that have no dependencies
        let mut independent: Vec<(SourceReference, ReferenceMetadata)> = self
            .references
            .extract_if(|_, metadata| metadata.dependencies.is_empty())
            .collect();
        // All remaining references that have unsorted dependencies
        let mut dependents = self.references;

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
                    references: dependents
                        .into_keys()
                        .map(|reference| reference.reference)
                        .sorted()
                        .collect(),
                },
                location,
            })
        } else {
            Ok(SortedGraph {
                references: sorted,
                document_map: self.document_map,
            })
        }
    }
}

/// Same as [UnsortedGraph], but the references have been sorted topologically
/// so that each reference only has dependencies on the references before it
/// in the map.
struct SortedGraph<'input> {
    references: IndexMap<SourceReference, ReferenceMetadata<'input>>,
    document_map: DocumentMap<'input>,
}

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
    fn replace(
        mut self,
        source_map: &SourceMap,
    ) -> Result<SourcedYaml<'input>> {
        // For each reference in the document, resolve it any replace all
        // instances of the reference with the resolved value. Because the
        // references are in topological order, we know each one will only
        // have dependencies on its predecessors. Because we do each replacement
        // inline, the dependencies will automatically be resolved in their
        // dependents.
        for (reference, metadata) in self.references {
            // Find the YAML value that this reference points to. Clone is
            // necessary to release the ref on `self`
            let resolved = reference.resolve(&self.document_map)?.clone();

            for path in metadata.locations {
                // Find the document containing the reference
                let document = self.document_map.get_mut(path.source_id);
                // Then find the referencing value within that doc
                let value = path.get(document);
                // The scanning process ensures that each path points to a
                // mapping containing a `$ref`
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
                                reference: reference.reference,
                                parent: value.location.resolve(source_map),
                            },
                            location: resolved.location,
                        });
                    };
                    spread_mapping(mapping, to_spread);
                }
            }
        }
        Ok(self.document_map.root)
    }
}

/// Traverse a YAML value according a reference path, returning the value at the
/// end of the rainbow. If not found, return the source location of the deepest
/// value we successfully traversed, which is also the location of
/// where the path went cold.
fn path_lookup<'input, 'value>(
    value: &'value SourcedYaml<'input>,
    path: &[String],
) -> std::result::Result<&'value SourcedYaml<'input>, SourceIdLocation> {
    if let [first, rest @ ..] = path {
        let location = value.location;
        // We need to go deeper. Value better be something we can drill into
        match &value.data {
            YamlData::Value(_) => Err(location),
            YamlData::Sequence(sequence) => {
                // Parse the segment as an int. If parsing fails, we can
                // report this as a generic "no resource" error, as there
                // isn't a traversable resource at the given path
                let index: usize = first.parse().or(Err(location))?;
                let inner = sequence.get(index).ok_or(location)?;
                path_lookup(inner, rest)
            }
            YamlData::Mapping(mapping) => {
                let inner = mapping
                    // Clone is necessary to prevent lifetime fuckery. With
                    // &str, the lifetime of `first` gets promoted to the
                    // lifetime param on SourcedYaml, which is 'input
                    .get(&SourcedYaml::value_from_string(first.to_owned()))
                    .ok_or(location)?;
                path_lookup(inner, rest)
            }
            YamlData::Tagged(_, value) => {
                // Remove the tag and try again
                path_lookup(value, rest)
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
    mapping: &mut AnnotatedMapping<SourcedYaml<'input>>,
    to_spread: AnnotatedMapping<SourcedYaml<'input>>,
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

/// A parsed reference with its *source* resolved. The source ID is stored so
/// we can easily look up the referenced document from [DocumentMap].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SourceReference {
    reference: Reference,
    source_id: SourceId,
}

impl SourceReference {
    /// Find the value referred to by this reference
    fn resolve<'value, 'input>(
        &self,
        document_map: &'value DocumentMap<'input>,
    ) -> Result<&'value SourcedYaml<'input>> {
        // Find the referenced document from our source ID
        let document = document_map.get(self.source_id);
        path_lookup(document, &self.reference.path).map_err(|location| {
            LocatedError {
                error: ReferenceError::NoResource(self.reference.clone()),
                // Error location points to the deepest value we were able to
                // traverse. Using the location of the reference may be more
                // intuitive, but this is easier to get and it points the user
                // to where the reference ceased to be valid
                location,
            }
        })
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
        // Sources must match
        self.source_id == location.source_id
        // Look for parent/equal/child
            && self
                .reference
                .path
                .iter()
                .zip(location.segments.iter())
                .all(|(ref_part, location_part)| {
                    location_part == ref_part.as_str()
                })
    }
}

/// A pointer to a YAML value. The reference points to a particular YAML
/// document (`source`) and a path to the value within that document (`path`).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Reference {
    /// Pointer to a YAML document. Everything before the `#` in the URI
    source: ReferenceSource,
    /// Pointer to a particular value within the source document. Everything
    /// after the `#` in the URI
    path: Vec<String>,
}

impl Reference {
    /// Attempt to parse a YAML value as a reference. This should be the value
    /// assigned to the `$ref` key, *not* the parent mapping. The value must be
    /// a string that parses as a valid URI.
    fn try_from_yaml(value: &SourcedYaml) -> Result<Self> {
        // We can hit two error cases:
        // - It's not a string and therefore can't be parsed
        // - It's a string but can't parse into a reference
        if let YamlData::Value(Scalar::String(reference)) = &value.data {
            let reference =
                reference.parse::<Reference>().map_err(|error| {
                    LocatedError {
                        error,
                        location: value.location,
                    }
                })?;
            Ok(reference)
        } else {
            Err(LocatedError {
                error: ReferenceError::NotAReference,
                location: value.location,
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

        let source =
            |input: &mut &str| -> ModalResult<ReferenceSource, EmptyError> {
                alt((
                    take_until(1.., "#")
                        .map(|path: &str| ReferenceSource::File(path.into())),
                    "".map(|_| ReferenceSource::Local),
                ))
                .parse_next(input)
            };

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

/// A reference's source is everything before the `#` in the URI. It defines
/// which YAML document the reference is pointing to.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ReferenceSource {
    /// Reference another value within the same YAML document
    Local,
    /// Reference to another file on the local file system. The file path can
    /// be relative or absolute. If relative, it will be resolved relative to
    /// the parent dir of the referencing file.
    File(PathBuf),
}

impl Display for ReferenceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            // Local source is blank
            ReferenceSource::Local => Ok(()),
            ReferenceSource::File(path) => {
                write!(f, "{}", path.display())
            }
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
    /// YAML file ID
    source_id: SourceId,
    segments: Vec<YamlPathSegment<'input>>,
    /// Source location of the *value* associated with this path. Used for
    /// error messages
    location: SourceIdLocation,
}

impl<'input> YamlPath<'input> {
    fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            segments: Vec::new(),
            location: SourceIdLocation::default(),
        }
    }

    /// Add a segment to this path, returning a new path
    fn cons(
        &self,
        segment: impl Into<YamlPathSegment<'input>>,
        location: SourceIdLocation,
    ) -> Self {
        let mut segments = self.segments.clone();
        segments.push(segment.into());
        Self {
            source_id: self.source_id,
            segments,
            location,
        }
    }

    /// Extract a sub-value from a root YAML value by traversing according to
    /// this path
    fn get<'value>(
        &self,
        root: &'value mut SourcedYaml<'input>,
    ) -> &'value mut SourcedYaml<'input> {
        let mut value: &'value mut SourcedYaml<'input> = root;
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
    Mapping(SourcedYaml<'input>),
}

#[cfg(test)]
impl From<&'static str> for YamlPathSegment<'static> {
    fn from(value: &'static str) -> Self {
        Self::Mapping(SourcedYaml::value_from_str(value))
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

/// Map of all YAML document that have been loaded
#[derive(Debug)]
struct DocumentMap<'input> {
    /// Root value is stored in its own field since it's always present and may
    /// be from a file or a static YAML string
    root: SourcedYaml<'input>,
    /// ID of the source that the root value was extracted from. Could be
    /// either a file path or [SourceId::Memory]
    root_source_id: SourceId,
    /// Additional sources (imported via reference) are stored keyed by their
    /// absolute path. Source IDs should all be files, as there's no way to
    /// reference an additional in-memory document.
    additional: HashMap<SourceId, SourcedYaml<'input>>,
}

impl<'input> DocumentMap<'input> {
    fn get(&self, source_id: SourceId) -> &SourcedYaml<'input> {
        if source_id == self.root_source_id {
            &self.root
        } else {
            // All sources should be added to the map when the references
            // are first encountered, so we expect everything to be present
            self.additional.get(&source_id).unwrap_or_else(|| {
                panic!("Unregistered source `{source_id:?}`")
            })
        }
    }

    fn get_mut(&mut self, source_id: SourceId) -> &mut SourcedYaml<'input> {
        if source_id == self.root_source_id {
            &mut self.root
        } else {
            // All sources should be added to the map when the references
            // are first encountered, so we expect everything to be present
            self.additional.get_mut(&source_id).unwrap_or_else(|| {
                panic!("Unregistered source `{source_id:?}`")
            })
        }
    }
}

/// Error while parsing or resolving a reference
#[derive(Debug, Error)]
pub enum ReferenceError {
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
        referring parent at {parent} is a mapping with multiple fields. \
        The referenced value must also be a mapping so it can be spread into \
        the referring map"
    )]
    ExpectedMapping {
        reference: Reference,
        parent: SourceLocation,
    },

    /// Failed to parse a string to a reference
    #[error("Invalid reference: `{0}`")]
    InvalidReference(String),

    /// Error while loading another source for a reference
    #[error(transparent)]
    Nested(Box<super::YamlErrorKind>),

    /// Reference parsed correctly but doesn't point to a resource
    #[error("Resource does not exist: `{0}`")]
    NoResource(Reference),

    /// YAML value is not a string and therefore can't be a reference
    #[error("Not a reference")]
    // Could potentially include the invalid value here, shortcutting for now
    NotAReference,

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
    use crate::{TempDir, assert_err, temp_dir};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
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
    fn test_reference(#[case] input: &str, #[case] expected: &str) {
        let input = parse_yaml(input);
        let expected = parse_yaml(expected);
        assert_eq!(
            input.resolve_references(&mut SourceMap::default()).unwrap(),
            expected
        );
    }

    /// Load a reference to another file
    #[rstest]
    // Reference a file via relative path
    #[case::relative(
        r#"
value:
    $ref: "./file1.yml#/value"
"#,
        &[("file1.yml", "value: test")],
        "value: test",
    )]
    // Reference a file via absolute path ({ROOT} gets replace with temp dir)
    #[case::absolute(
        r#"
value:
    $ref: "{ROOT}/file1.yml#/value"
"#,
        &[("file1.yml", "value: test")],
        "value: test",
    )]
    // The local reference source in a subfile should be resolved within that
    // document instead of the root
    #[case::local_in_subfile(
        r#"
value:
    $ref: "./file1.yml#/indirection"
"#,
        // This inner reference should refer to file1.yml, NOT the root document
        &[("file1.yml", r##"
value: test
indirection:
    $ref: "#/value"
        "##)],
        "value: test",
    )]
    // Create two files that reference each other. The references are
    // non-cyclic so it doesn't create an error
    // file1/r2 -> file2/r1 -> file1/r1
    #[case::multi_file(
        r#"
value:
    $ref: "./file1.yml#/requests/r2/url"
"#,
        &[
            (
                "file1.yml",
                r#"
requests:
    r1:
        url: test
    r2:
        url: {"$ref": "file2.yml#/requests/r1/url"}
"#,
            ),
            (
                "file2.yml",
                r#"
requests:
    r1:
        url: {"$ref": "file1.yml#/requests/r1/url"}
"#
            ),
        ],
        r#"{"value": "test"}"#
    )]
    fn test_reference_file(
        temp_dir: TempDir,
        #[case] root: &str,
        #[case] additional: &[(&str, &str)], // list of (path, yaml)
        #[case] expected: &str,
    ) {
        for (path, yaml) in additional {
            fs::write(temp_dir.join(path), yaml).unwrap();
        }

        // Windows paths include backslashes, which we need to escape to keep
        // them as valid YAML
        let base_dir = temp_dir.to_str().unwrap().replace('\\', "\\\\");
        let input = parse_yaml(
            // Inject the temp dir into paths as needed
            &root.replace("{ROOT}", &base_dir),
        );
        let expected = parse_yaml(expected);

        // Fake a path for the root value so imports will be in the temp dir
        let mut source_map = SourceMap::default();
        source_map.add_source(temp_dir.join("root.yml"));

        let actual = input.resolve_references(&mut source_map).unwrap();
        assert_eq!(actual, expected);
    }

    /// Cross-file reference cycles should be detected and throw an error
    #[rstest]
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
        let input = parse_yaml(&yaml);

        // Fake a path for the root value so imports will be in the temp dir
        let mut source_map = SourceMap::default();
        source_map.add_source(temp_dir.join("root.yml"));
        let result = input.resolve_references(&mut source_map);

        assert_err!(
            result.map_err(|error| error.error),
            "References contain one or more cycles: \
            file1.yml#/data, file2.yml#/data"
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
    #[case::io(
        r#"
        root:
            $ref: "./other.yml#/root"
        "#,
        // Strategically omit the base of the path because it's absolute
        if cfg!(unix) {
            "other.yml: No such file or directory"
        } else {
            "other.yml: The system cannot find the file specified"
        }
    )]
    fn test_errors(
        temp_dir: TempDir,
        #[case] input: &str,
        #[case] expected_error: &str,
    ) {
        let input = parse_yaml(input);

        // Fake a path for the root value so imports will be in the temp dir
        let mut source_map = SourceMap::default();
        source_map.add_source(temp_dir.join("root.yml"));

        let result = input.resolve_references(&mut source_map);
        assert_err(result.map_err(LocatedError::into_error), expected_error);
    }

    /// Test [Reference::depends_on]
    #[rstest]
    #[case::ref_below(SourceId::Memory, "#/a", vec!["a", "b"], true)]
    #[case::ref_at(SourceId::Memory, "#/a/b", vec!["a", "b"], true)]
    #[case::ref_above(SourceId::Memory, "#/a/b/c", vec!["a", "b"], true)]
    #[case::disjoint(SourceId::Memory, "#/a/c", vec!["a", "b"], false)]
    // Same paths, different sources
    #[case::different_sources(SourceId::File(0), "#/a/b", vec!["a", "b"], false)]
    fn test_depends_on(
        #[case] reference_source_id: SourceId,
        #[case] reference: &str,
        #[case] segments: Vec<&'static str>,
        #[case] is_child: bool,
    ) {
        let path = YamlPath {
            source_id: SourceId::Memory,
            segments: segments.into_iter().map(YamlPathSegment::from).collect(),
            location: SourceIdLocation::default(),
        };
        let reference = SourceReference {
            source_id: reference_source_id,
            reference: reference.parse::<Reference>().unwrap(),
        };
        assert_eq!(reference.depends_on(&path), is_child);
    }

    #[rstest]
    #[case::local("#/a/b", ReferenceSource::Local, vec!["a", "b"])]
    #[case::num_index("#/a/0", ReferenceSource::Local, vec!["a", "0"])]
    #[case::file_local(
        "./file.yml#/a/b",
        ReferenceSource::File("./file.yml".into()),
        vec!["a", "b"],
    )]
    #[case::file_absolute(
        "/root/file.yml#/a/b",
        ReferenceSource::File("/root/file.yml".into()),
        vec!["a", "b"],
    )]
    #[case::file_tilde(
        "~/file.yml#/a/b",
        // This won't be expanded during parsing but it should parse
        ReferenceSource::File("~/file.yml".into()),
        vec!["a", "b"],
    )]
    fn test_parse_reference(
        #[case] reference: &str,
        #[case] expected_source: ReferenceSource,
        #[case] expected_path: Vec<&str>,
    ) {
        let actual = reference.parse::<Reference>().unwrap();
        let expected = Reference {
            source: expected_source,
            path: expected_path.into_iter().map(String::from).collect(),
        };
        assert_eq!(actual, expected);
    }

    fn parse_yaml(yaml: &str) -> SourcedYaml<'static> {
        SourcedYaml::load_from_str(yaml, SourceId::Memory).unwrap()
    }
}
