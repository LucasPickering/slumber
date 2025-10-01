// Procedural macros for Slumber

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ItemFn, Pat, PatType, parse_macro_input};

/// Procedural macro to convert a plain function into a template function.
///
/// The given function can take any number of arguments, as long as each one
/// can be converted from `Value`. It can return any output as long as it can be
/// converted to `Result<Value, RenderError>`. The function can be sync or
/// `async`.
///
/// By default, arguments to the function are extracted and supplied as
/// positional arguments from the template function call, using the type's
/// `TryFromValue` implementation to convert from `Value`. This can be
/// customized using a set of attributes on each argument:
/// - `#[context]` - Pass the template context value. Cannot be combined with
///   other attributes, and at most one argument can have this attribute.
/// - `#[kwarg]` - Extract a keyword argument with the same name as the argument
/// - `#[serde]` - Use the type's `Deserialize` implementation to convert from
///   `Value`, instead of `TryFromValue`. Can be used alone for positional
///   arguments, or combined with `#[kwarg]` for keyword arguments.
#[proc_macro_attribute]
pub fn template(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // The input fn will be replaced by a wrapper, and it will be moved into a
    // definition within the wrapper
    let mut inner_fn = parse_macro_input!(item as ItemFn);

    // Grab metadata from the input fn, then modify it
    let vis = inner_fn.vis.clone();
    let original_fn_ident = inner_fn.sig.ident.clone();
    let inner_fn_ident = format_ident!("{}_inner", original_fn_ident);
    inner_fn.sig.ident = inner_fn_ident.clone();
    inner_fn.vis = syn::Visibility::Inherited;

    // Gather argument info and strip custom attributes for the inner function
    let arg_infos: Vec<ArgumentInfo> = inner_fn
        .sig
        .inputs
        .iter_mut()
        .filter_map(|input| match input {
            FnArg::Receiver(_) => None,
            // This will scan the argument for relevant attributes, and remove
            // them as they're consumed
            FnArg::Typed(pat_type) => ArgumentInfo::from_pattern(pat_type),
        })
        .collect();

    // Determine context type. If an arg has #[context], use that. Otherwise
    // add a generic param because we can accept any context type.
    let context_type_param = if let Some(context_info) = arg_infos
        .iter()
        .find(|info| matches!(info.kind, ArgumentKind::Context))
    {
        // Extract the type from the context parameter, handling references
        let context_type = match &context_info.type_name {
            syn::Type::Reference(type_ref) => &*type_ref.elem,
            other_type => other_type,
        };
        quote! { #context_type }
    } else {
        // No context parameter found, use generic T
        quote! { T }
    };

    // Add generic parameter if no context param exists
    let generic_param = if arg_infos
        .iter()
        .any(|info| matches!(info.kind, ArgumentKind::Context))
    {
        quote! {}
    } else {
        quote! { <T> }
    };

    // Generate one statement per argument to extract each one
    let argument_extracts = arg_infos.iter().map(ArgumentInfo::extract);

    let call_args = arg_infos.iter().map(|info| {
        let name = &info.name;
        quote! { #name }
    });

    // If the function is async, we'll need to include that on the outer
    // function and also inject a .await
    let asyncness = inner_fn.sig.asyncness;
    let await_inner = if asyncness.is_some() {
        quote! { .await }
    } else {
        quote! {}
    };

    quote! {
        #vis #asyncness fn #original_fn_ident #generic_param (
            #[allow(unused_mut)]
            mut arguments: ::slumber_template::Arguments<'_, #context_type_param>
        ) -> ::core::result::Result<
            ::slumber_template::LazyValue,
            ::slumber_template::RenderError
        > {
            #inner_fn

            #(#argument_extracts)*
            // Make sure there were no extra arguments passed in
            arguments.ensure_consumed()?;
            let output = #inner_fn_ident(#(#call_args),*) #await_inner;
            ::slumber_template::FunctionOutput::into_result(output)
        }
    }
    .into()
}

/// Metadata about a parameter to the template function
struct ArgumentInfo {
    name: Ident,
    kind: ArgumentKind,
    type_name: syn::Type,
}

impl ArgumentInfo {
    /// Detect the argument name and kind from its pattern. This will modify the
    /// pattern to remove any recognized attributes.
    fn from_pattern(pat_type: &mut PatType) -> Option<Self> {
        let pat_ident = match &*pat_type.pat {
            Pat::Ident(pat_ident) => pat_ident.ident.clone(),
            _ => return None,
        };

        // Remove known attributes from this arg. Any unrecognized attributes
        // will be left because they may be from other macros.
        let mut attributes = ArgumentAttributes::default();
        pat_type.attrs.retain(|attr| {
            // Retain any attribute that we don't recognize
            if let Some(ident) = attr.path().get_ident() {
                !attributes.add(ident)
            } else {
                true
            }
        });
        let kind = ArgumentKind::from_attributes(attributes);

        Some(Self {
            name: pat_ident,
            kind,
            type_name: (*pat_type.ty).clone(),
        })
    }

    /// Generate code to extract this argument from an Arguments value
    fn extract(&self) -> proc_macro2::TokenStream {
        let name = &self.name;
        match self.kind {
            ArgumentKind::Context => quote! {
                let #name = arguments.context();
            },
            ArgumentKind::Positional => quote! {
                let #name = arguments.pop_position()?;
            },
            ArgumentKind::Kwarg => {
                let key = name.to_string();
                quote! {
                    let #name = arguments.pop_keyword(#key)?;
                }
            }
        }
    }
}

/// Track what attributes are on a function argument
#[derive(Default)]
struct ArgumentAttributes {
    /// `#[context]` attribute is present
    context: bool,
    /// `#[kwarg]` attribute is present
    kwarg: bool,
}

impl ArgumentAttributes {
    /// Enable the given attribute. Return false if it's an unknown attribute
    fn add(&mut self, ident: &Ident) -> bool {
        match ident.to_string().as_str() {
            "context" => {
                self.context = true;
                true
            }
            "kwarg" => {
                self.kwarg = true;
                true
            }
            _ => false,
        }
    }
}

/// The kind of an argument defines how it should be extracted
enum ArgumentKind {
    /// Extract template context
    Context,
    /// Default (no attribute) - Extract next positional argument and convert it
    /// using its `TryFromValue` implementation
    Positional,
    /// Extract keyword argument matching the parameter name and convert it
    /// using its `TryFromValue` implementation
    Kwarg,
}

impl ArgumentKind {
    /// From the set of attributes on a parameter, determine how it should be
    /// extracted
    fn from_attributes(attributes: ArgumentAttributes) -> Self {
        match attributes {
            ArgumentAttributes {
                context: false,
                kwarg: false,
            } => Self::Positional,
            ArgumentAttributes {
                context: true,
                kwarg: false,
            } => Self::Context,
            ArgumentAttributes {
                context: false,
                kwarg: true,
            } => Self::Kwarg,
            ArgumentAttributes { context: true, .. } => {
                panic!("#[context] cannot be used with other attributes")
            }
        }
    }
}
