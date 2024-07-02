//! The OpenAPI specification supports `$ref`s, their own home-rolled version of
//! YAML's aliases.
//!
//! Spec: <https://spec.openapis.org/oas/v3.0.3#reference-object>
//!
//! This module is an implementation of an easily-extendable resolver for
//! components stored inside an OpenAPI specifications.

use indexmap::IndexMap;
use openapiv3::{
    Components, Example, Parameter, ReferenceOr, RequestBody, Schema,
    SecurityScheme,
};
use std::borrow::Cow;
use thiserror::Error;
use winnow::{
    combinator::{preceded, rest},
    error::ErrorKind,
    Parser,
};

/// Helper struct for resolving references within a single OpenAPI spec. This
/// does *not* resolve references across multiple files.
pub struct ReferenceResolver(Components);

/// An error that can occur while resolving a reference
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("`{_0}` refers to an object that does not exist in the schema")]
    UnknownReference(String),
    #[error(
        "`{reference}` is an invalid reference for type `{expected_type}`"
    )]
    InvalidReference {
        reference: String,
        expected_type: &'static str,
    },
}

/// Abstraction for the various types of components that we can resolve
/// references too. Each component is contained under its own subfield of
/// [Components]; this trait handles that access.
pub trait ComponentKind: Sized {
    /// The name of this type used in references, e.g.
    /// `#/components/<type_name>/foo`
    const TYPE_NAME: &'static str;

    /// Get the map of components of this particular type. This extracts a
    /// single static field from the component object
    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>>;
}

impl ComponentKind for Example {
    const TYPE_NAME: &'static str = "examples";

    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>> {
        &components.examples
    }
}

impl ComponentKind for Parameter {
    const TYPE_NAME: &'static str = "parameters";

    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>> {
        &components.parameters
    }
}

impl ComponentKind for RequestBody {
    const TYPE_NAME: &'static str = "requestBodies";

    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>> {
        &components.request_bodies
    }
}

impl ComponentKind for Schema {
    const TYPE_NAME: &'static str = "schemas";

    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>> {
        &components.schemas
    }
}

impl ComponentKind for SecurityScheme {
    const TYPE_NAME: &'static str = "securitySchemes";

    fn get_components(
        components: &Components,
    ) -> &IndexMap<String, ReferenceOr<Self>> {
        &components.security_schemes
    }
}

impl ReferenceResolver {
    pub fn new(components: Option<Components>) -> Self {
        Self(components.unwrap_or_default())
    }

    /// Get a component of a particular type by just its name. This is useful
    /// in a small set of scenarios, where simple names are used
    pub fn get_by_name<T: ComponentKind>(
        &self,
        name: &str,
    ) -> Result<&T, ResolveError> {
        let ref_or_component = self
            .get_component::<T>(name)
            .ok_or_else(|| ResolveError::UnknownReference(name.to_string()))?;
        match ref_or_component {
            ReferenceOr::Item(item) => Ok(item),
            ReferenceOr::Reference { reference } => {
                self.get_by_reference(reference)
            }
        }
    }

    /// Resolve a [ReferenceOr] into the contained item. If the item is already
    /// there, just unwrap it and returned the owned value. If it's a reference,
    /// resolve it and return a reference to the data.
    pub fn resolve<T: Clone + ComponentKind>(
        &self,
        reference_or: ReferenceOr<T>,
    ) -> Result<Cow<'_, T>, ResolveError> {
        match reference_or {
            ReferenceOr::Item(item) => Ok(Cow::Owned(item)),
            ReferenceOr::Reference { reference } => {
                self.get_by_reference(&reference).map(Cow::Borrowed)
            }
        }
    }

    /// Resolve a reference URI. The reference must refer to an object of a
    /// statically known type (`T`), and must be in the same file.
    fn get_by_reference<T: ComponentKind>(
        &self,
        reference: &str,
    ) -> Result<&T, ResolveError> {
        let name = parse_reference::<T>(reference)?;

        let ref_or_component =
            self.get_component::<T>(name).ok_or_else(|| {
                ResolveError::UnknownReference(reference.to_owned())
            })?;

        match ref_or_component {
            ReferenceOr::Item(item) => Ok(item),
            // RECURSION
            ReferenceOr::Reference { reference } => {
                self.get_by_reference(reference)
            }
        }
    }

    /// Get a component of a particular type by name
    fn get_component<T: ComponentKind>(
        &self,
        name: &str,
    ) -> Option<&ReferenceOr<T>> {
        T::get_components(&self.0).get(name)
    }
}

/// Parse a reference string for a particular type of component. Return just
/// the name of the individual resource.
fn parse_reference<T: ComponentKind>(
    reference: &str,
) -> Result<&str, ResolveError> {
    // This reutrns a pretty unhelpful error if the reference has a file at the
    // beginning. It's a "valid" reference but we don't know how to parse it.
    // These references are supposed to be valid URIs so we could use a URI
    // parser instead.
    let mut parser = preceded(
        ("#/components/", T::TYPE_NAME, "/").void(),
        rest::<&str, ErrorKind>,
    );
    parser
        .parse(reference)
        .map_err(|_| ResolveError::InvalidReference {
            reference: reference.to_string(),
            expected_type: T::TYPE_NAME,
        })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::test_util::assert_err;
    use indexmap::{indexmap, IndexMap};
    use openapiv3::{Components, OAuth2Flows};
    use rstest::rstest;

    #[rstest]
    #[case::simple("petstore_auth")]
    #[case::nested("nested")]
    fn test_resolve_by_name(#[case] name: &str) {
        let petstore_auth = SecurityScheme::OAuth2 {
            flows: OAuth2Flows {
                implicit: None,
                password: None,
                client_credentials: None,
                authorization_code: None,
                extensions: IndexMap::default(),
            },
            description: None,
            extensions: IndexMap::default(),
        };
        let nested_reference = ReferenceOr::Reference {
            reference: "#/components/securitySchemes/petstore_auth".to_owned(),
        };
        let security_schemes = indexmap! {
            "petstore_auth".to_string() => ReferenceOr::Item(petstore_auth.clone()),
            "nested".to_string() => nested_reference,
        };
        let resolver = ReferenceResolver::new(Some(Components {
            security_schemes,
            ..Default::default()
        }));

        // All the test cases should resolve to the same value
        let resolved = resolver.get_by_name::<SecurityScheme>(name);
        assert_eq!(resolved, Ok(&petstore_auth));
    }

    #[test]
    fn test_resolve_by_name_error() {
        let resolver = ReferenceResolver::new(Some(Components::default()));
        let result =
            resolver.get_by_name::<SecurityScheme>("auth_that_does_not_exist");
        assert_err!(result, "refers to an object that does not exist")
    }

    #[rstest]
    #[case::simple("#/components/requestBodies/pet")]
    #[case::nested("#/components/requestBodies/nested")]
    fn test_resolve_by_reference(#[case] reference: &str) {
        let request_body = RequestBody {
            description: None,
            content: IndexMap::default(),
            required: true,
            extensions: IndexMap::default(),
        };
        let nested_reference = ReferenceOr::Reference {
            reference: "#/components/requestBodies/pet".to_owned(),
        };
        let components = ReferenceResolver::new(Some(Components {
            request_bodies: indexmap! {
                "pet".to_string() => ReferenceOr::Item(request_body.clone()),
                "nested".to_string() => nested_reference,
            },
            ..Default::default()
        }));

        let resolved = components.get_by_reference::<RequestBody>(reference);
        assert_eq!(resolved, Ok(&request_body));
    }

    #[rstest]
    #[case::unknown_target(
        "#/components/requestBodies/fake",
        "refers to an object that does not exist"
    )]
    #[case::wrong_type(
        "#/components/parameters/fake",
        "is an invalid reference for type `requestBodies`"
    )]
    #[case::another_file(
        "other_file.yml#/components/requestBodies/fake",
        "is an invalid reference for type `requestBodies`"
    )]
    fn test_resolve_by_reference_error(
        #[case] reference: &str,
        #[case] expected_error: &str,
    ) {
        let components = Components::default();
        let components = ReferenceResolver::new(Some(components));

        let result = components.get_by_reference::<RequestBody>(reference);
        assert_err!(result, expected_error);
    }
}
