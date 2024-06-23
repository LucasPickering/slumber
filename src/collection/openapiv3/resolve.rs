//! The OpenAPI specification supports `$ref`s, their own home-rolled version of YAML's aliases.
//!
//! Refer to the specification : https://spec.openapis.org/oas/v3.0.3#reference-object
//!
//! This module is an implementation of an easily-extendable resolver for components stored inside
//! an OpenAPI specifications.

use openapiv3::{
    Components, Parameter, ReferenceOr, RequestBody, SecurityScheme,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum OpenAPIResolveError {
    #[error("The given OpenAPIv3 specs do not contain the `components` field")]
    MissingComponentsObject,
    #[error("Could not find the security scheme {_0} inside components.securitySchemes")]
    SecuritySchemeNotFound(String),
    #[error(
        "Could not find the request body {_0} inside components.requestBodies"
    )]
    RequestBodyNotFound(String),
    #[error("Could not find the parameter {_0} inside components.parameters")]
    ParameterNotFound(String),
    #[error("Could not resolve the reference {_0}")]
    UnhandledReference(String),
    #[error("Tried to resolve an unsupported component {_0}. This is a bug, please open an issue.")]
    UnhandledComponentKind(String),
}

pub(super) struct OpenApiReferenceResolver(Option<Components>);

pub(super) struct OpenApiComponentReference<'a> {
    component_kind: OpenApiResolvableComponentKind,
    value: &'a str,
}

enum OpenApiResolvableComponentKind {
    SecurityScheme,
    RequestBody,
    Parameter,
}

impl<'a> TryFrom<&'a str> for OpenApiResolvableComponentKind {
    type Error = OpenAPIResolveError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "securitySchemes" => {
                Ok(OpenApiResolvableComponentKind::SecurityScheme)
            }
            "requestBodies" => Ok(OpenApiResolvableComponentKind::RequestBody),
            "parameters" => Ok(OpenApiResolvableComponentKind::Parameter),
            _ => Err(OpenAPIResolveError::UnhandledComponentKind(
                value.to_string(),
            )),
        }
    }
}

const REFERENCE_PREFIX: &str = "#/components/";
impl<'a> TryFrom<&'a str> for OpenApiComponentReference<'a> {
    type Error = OpenAPIResolveError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        // We currently do not support parsing references that aren't internal to the provided OpenAPI spec
        if !value.starts_with(REFERENCE_PREFIX) {
            return Err(OpenAPIResolveError::UnhandledReference(
                value.to_string(),
            ));
        }
        let (_, component) =
            value.split_once(REFERENCE_PREFIX).ok_or_else(|| {
                OpenAPIResolveError::UnhandledReference(value.to_string())
            })?;
        let (component_kind, value) =
            component.split_once('/').ok_or_else(|| {
                OpenAPIResolveError::UnhandledReference(value.to_string())
            })?;
        let component_kind =
            OpenApiResolvableComponentKind::try_from(component_kind)?;
        Ok(OpenApiComponentReference {
            component_kind,
            value,
        })
    }
}

macro_rules! impl_resolver_direct_lookup {
    ($func_name:ident, $openapi_ty:ident, $component_lookup_ident:ident, $resolve_error_variant:ident) => {
        /// Does a lookup to the internal components of the specifications trying to find
        /// the given reference.
        ///
        /// This function does not parse the path to the component, the name is used as-is for the
        /// lookup in the map.
        pub fn $func_name(
            &self,
            reference: &str,
        ) -> Result<&$openapi_ty, OpenAPIResolveError> {
            let ref_or_component =
                self.get_components().and_then(|components| {
                    components
                        .$component_lookup_ident
                        .get(reference)
                        .ok_or_else(|| {
                            OpenAPIResolveError::$resolve_error_variant(
                                reference.to_string(),
                            )
                        })
                })?;
            match ref_or_component {
                ReferenceOr::Item(item) => Ok(item),
                ReferenceOr::Reference { reference: _ } => {
                    Err(OpenAPIResolveError::UnhandledReference(
                        reference.to_string(),
                    ))
                }
            }
        }
    };
}

macro_rules! impl_resolver_parsing_reference {
    ($func_name:ident, $openapi_ty:ident, $component_lookup_ident:ident, $resolve_error_variant:ident) => {
        // Parses the given reference to the internal components of the specifications, and does a
        // lookup to find the relevant component.
        pub fn $func_name(
            &self,
            reference: &str,
        ) -> Result<&$openapi_ty, OpenAPIResolveError> {
            let parsed_reference =
                OpenApiComponentReference::try_from(reference)?;
            let value = match parsed_reference.component_kind {
                OpenApiResolvableComponentKind::$openapi_ty => {
                    Ok(parsed_reference.value)
                }
                // The parsed reference's kind did not match the type that the caller was expecting.
                _ => Err(OpenAPIResolveError::UnhandledReference(
                    reference.to_string(),
                )),
            }?;
            let ref_or_component =
                self.get_components().and_then(|components| {
                    components.$component_lookup_ident.get(value).ok_or_else(
                        || {
                            OpenAPIResolveError::$resolve_error_variant(
                                value.to_string(),
                            )
                        },
                    )
                })?;
            match ref_or_component {
                ReferenceOr::Item(item) => Ok(item),
                ReferenceOr::Reference { reference: _ } => {
                    Err(OpenAPIResolveError::UnhandledReference(
                        reference.to_string(),
                    ))
                }
            }
        }
    };
}

impl OpenApiReferenceResolver {
    pub fn new(components: Option<Components>) -> Self {
        OpenApiReferenceResolver(components)
    }

    fn get_components(&self) -> Result<&Components, OpenAPIResolveError> {
        self.0
            .as_ref()
            .ok_or(OpenAPIResolveError::MissingComponentsObject)
    }

    impl_resolver_direct_lookup!(
        get_security_scheme,
        SecurityScheme,
        security_schemes,
        SecuritySchemeNotFound
    );
    impl_resolver_parsing_reference!(
        get_request_body,
        RequestBody,
        request_bodies,
        RequestBodyNotFound
    );
    impl_resolver_parsing_reference!(
        get_parameter,
        Parameter,
        parameters,
        ParameterNotFound
    );
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use indexmap::IndexMap;
    use openapiv3::{Components, OAuth2Flows};

    #[test]
    fn test_resolve_security_scheme() {
        let mut security_schemes = IndexMap::default();
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
        security_schemes.insert(
            "petstore_auth".to_string(),
            ReferenceOr::Item(petstore_auth.clone()),
        );
        let components = Components {
            security_schemes,
            ..Default::default()
        };
        let components = OpenApiReferenceResolver::new(Some(components));

        let resolved = components.get_security_scheme("petstore_auth");
        assert_eq!(resolved, Ok(&petstore_auth));
    }

    #[test]
    fn test_do_not_resolve_security_scheme_that_does_not_exist() {
        let components = Components::default();
        let components = OpenApiReferenceResolver::new(Some(components));

        let not_resolved =
            components.get_security_scheme("auth_that_does_not_exist");
        assert_eq!(
            not_resolved,
            Err(OpenAPIResolveError::SecuritySchemeNotFound(
                "auth_that_does_not_exist".to_string()
            ))
        )
    }

    #[test]
    fn test_resolve_request_body() {
        let pet_request_body = RequestBody {
            description: None,
            content: IndexMap::default(),
            required: true,
            extensions: IndexMap::default(),
        };
        let mut request_bodies = IndexMap::default();
        request_bodies.insert(
            "Pet".to_string(),
            ReferenceOr::Item(pet_request_body.clone()),
        );
        let components = Components {
            request_bodies,
            ..Default::default()
        };
        let components = OpenApiReferenceResolver::new(Some(components));

        let resolved =
            components.get_request_body("#/components/requestBodies/Pet");
        assert_eq!(resolved, Ok(&pet_request_body));
    }

    #[test]
    fn test_do_not_resolve_request_body_that_does_not_exist() {
        let components = Components::default();
        let components = OpenApiReferenceResolver::new(Some(components));

        let not_resolved = components.get_request_body(
            "#/components/requestBodies/request_body_that_does_not_exist",
        );
        assert_eq!(
            not_resolved,
            Err(OpenAPIResolveError::RequestBodyNotFound(
                "request_body_that_does_not_exist".to_string()
            ))
        )
    }
}
