//! Generate documentation for template functions.

use anyhow::{Context, Result, anyhow};
use itertools::Itertools;
use std::{
    cell::LazyCell,
    collections::HashMap,
    fmt::{self, Display},
    fs,
};
use syn::{
    Attribute, FnArg, GenericArgument, ItemFn, Pat, PathArguments, ReturnType,
    Type, parse_quote,
};

pub const REPLACE: &str = "{{#template_functions}}";
/// Rust file containing template function definitions
const INPUT_FILE: &str = "crates/core/src/render/functions.rs";
/// Strings that must appear in every doc comment. Use bold instead of headers
/// to keep clutter out of the Table of Contents
const REQUIRED_SECTIONS: &[&str] = &["**Parameters**", "**Examples**"];

thread_local! {
    /// Mapping to convert Rust types to template type names for the user
    ///
    /// This needs to be thread-local because `Type` is `!Sync`
    static TYPE_MAP: LazyCell<HashMap<Type, TypeDef>> = LazyCell::new(type_map);
}

/// Parse template functions from the specified source file, returning renderer
/// markdown
pub fn render() -> Result<String> {
    let content = fs::read_to_string(INPUT_FILE)
        .context(format!("Error reading {INPUT_FILE}"))?;

    let ast = syn::parse_file(&content)
        .context(format!("Error reading {INPUT_FILE}"))?;

    let functions = ast
        .items
        .into_iter()
        .filter_map(|item| {
            if let syn::Item::Fn(func) = item
                && has_template_attribute(&func.attrs)
            {
                Some(TemplateFunctionMetadata::from_item_fn(func))
            } else {
                None
            }
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(functions.into_iter().join("\n"))
}

/// Information about a Rust function
#[derive(Debug, Clone)]
struct TemplateFunctionMetadata {
    name: String,
    /// Doc comment is required. If one is missing, we'll return an error
    doc_comment: String,
    parameters: Vec<ParameterMetadata>,
    return_type: TypeDef,
}

impl TemplateFunctionMetadata {
    /// Load function metadata from the given definition
    fn from_item_fn(func: ItemFn) -> Result<Self> {
        let name = func.sig.ident.to_string();
        let doc_comment = Self::get_doc_comment(&func)?;
        let parameters = Self::get_parameters(&func.sig.inputs)?;
        let return_type = Self::get_return_type(&func.sig.output)?;

        Ok(TemplateFunctionMetadata {
            name,
            doc_comment,
            parameters,
            return_type,
        })
    }

    /// Get the doc comment from a function. If the doc comment is empty or
    /// is missing the required sections, return an error
    fn get_doc_comment(func: &ItemFn) -> Result<String> {
        let doc = func
            .attrs
            .iter()
            .filter_map(|attr| {
                if let Ok(meta) = attr.meta.require_name_value()
                    && attr.path().is_ident("doc")
                    && let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(lit_str),
                        ..
                    }) = &meta.value
                {
                    // Remove leading space on each line. The formatting
                    // doesn't need to be exact because we're going to
                    // be rendering as markdown
                    Some(format!("{}\n", lit_str.value().trim_start()))
                } else {
                    None
                }
            })
            .collect::<String>();

        if doc.is_empty() {
            return Err(anyhow!(
                "function `{}` is missing a doc comment",
                func.sig.ident
            ));
        }

        for required_section in REQUIRED_SECTIONS {
            if !doc.contains(required_section) {
                return Err(anyhow!(
                    "function `{}` is missing a `{}` section",
                    func.sig.ident,
                    required_section
                ));
            }
        }

        Ok(doc)
    }

    /// Extract parameters from a function signature
    fn get_parameters(
        inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>,
    ) -> Result<Vec<ParameterMetadata>> {
        inputs
            .iter()
            .filter_map(ParameterMetadata::from_fn_arg)
            .collect()
    }

    /// Extract return type from a function signature
    fn get_return_type(return_type: &ReturnType) -> Result<TypeDef> {
        match return_type {
            ReturnType::Default => Err(anyhow!("Function missing return type")),
            ReturnType::Type(_, ty) => TypeDef::get(ty),
        }
    }
}

impl Display for TemplateFunctionMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "### {name}

```typescript
{name}({parameters}): {return_type}
```

{doc}",
            name = self.name,
            parameters = self.parameters.iter().join(", "),
            return_type = self.return_type,
            doc = self.doc_comment,
        )
    }
}

/// Information about a Rust function parameter
#[derive(Debug, Clone)]
struct ParameterMetadata {
    name: String,
    type_def: TypeDef,
    is_kwarg: bool,
}

impl ParameterMetadata {
    fn from_fn_arg(input: &FnArg) -> Option<Result<Self>> {
        if let FnArg::Typed(pat_type) = input
            && let Pat::Ident(pat_ident) = pat_type.pat.as_ref()
            // Remove the context parameter, since it isn't passed by the user
            && !has_attr(&pat_type.attrs, "context")
        {
            let type_def = match TypeDef::get(&pat_type.ty) {
                Ok(type_def) => type_def,
                Err(error) => return Some(Err(error)),
            };
            Some(Ok(ParameterMetadata {
                name: pat_ident.ident.to_string(),
                type_def,
                is_kwarg: has_attr(&pat_type.attrs, "kwarg"),
            }))
        } else {
            None
        }
    }
}

impl Display for ParameterMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}: {}",
            self.name,
            if self.is_kwarg { "?" } else { "" },
            self.type_def
        )
    }
}

/// Does the function have a `#[template]` attribute?
fn has_template_attribute(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("template"))
}

/// Do any of the given attributes match a particular name?
fn has_attr(attrs: &[Attribute], target: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(target))
}

/// A minimal set of types, representing what the user will see in type
/// signatures. We map from Rust types to these using [TypeDef::get]
///
/// Recursive types use `&'static` to avoid boxing. It works because these are
/// all constructed by hand.
#[derive(Copy, Clone, Debug)]
enum TypeDef {
    /// Verdad o falso
    Boolean,
    /// Collection of bytes
    Bytes,
    /// Any string
    String,
    /// Only a specific string
    Literal(&'static str),
    /// Any template value
    Value,
    /// A homogenous list
    List(&'static Self),
    /// Any of a set of types
    Union(&'static [&'static Self]),
    /// A custom dynamic type. Typically a subset of `String`, such as
    /// `JsonPath`. The function docs must describe the type in detail
    Custom(&'static str),
}

impl TypeDef {
    /// Convert a Rust type to a known template function type
    fn get(ty: &Type) -> Result<Self> {
        // Extract Result::Ok or Option::Some, as those should be invisible to
        // the user
        let ty = if let Type::Path(path) = ty
            && let Some(segment) = path.path.segments.first()
            && (segment.ident == "Result" || segment.ident == "Option")
            && let PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(GenericArgument::Type(inner)) = args.args.first()
        {
            inner
        } else {
            ty
        };

        TYPE_MAP
            .with(|map| map.get(ty).copied())
            .ok_or_else(|| anyhow!("Unmapped type {ty:#?}"))
    }
}

impl Display for TypeDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boolean => write!(f, "boolean"),
            Self::Bytes => write!(f, "bytes"),
            Self::String => write!(f, "string"),
            Self::Literal(literal) => write!(f, "\"{literal}\""),
            Self::Value => write!(f, "value"),
            Self::List(inner) => write!(f, "{inner}[]"),
            Self::Union(members) => {
                write!(f, "{}", members.iter().format(" | "))
            }
            Self::Custom(custom) => write!(f, "{custom}"),
        }
    }
}

/// Build a map of Rust types to their corresponding TypeDefs
fn type_map() -> HashMap<Type, TypeDef> {
    /// Is this necessary? No!! But macros are fun :)
    macro_rules! union {
        ($($literals:literal)|* $(or $other:expr)?) => {
            TypeDef::Union(&[
                $(&TypeDef::Literal($literals)),*
                $($other)?
            ])
        };
    }

    HashMap::from_iter([
        (parse_quote!(bool), TypeDef::Boolean),
        (parse_quote!(Bytes), TypeDef::Bytes),
        (parse_quote!(JsonPath), TypeDef::Custom("JsonPath")),
        (parse_quote!(JsonPathValue), TypeDef::Value),
        (
            parse_quote!(JsonPathMode),
            union!("auto" | "single" | "array"),
        ),
        (parse_quote!(RecipeId), TypeDef::String),
        (
            parse_quote!(RequestTrigger),
            TypeDef::Union(&[
                &TypeDef::Literal("never"),
                &TypeDef::Literal("no_history"),
                &TypeDef::Literal("always"),
                &TypeDef::Custom("Duration"),
            ]),
        ),
        (parse_quote!(String), TypeDef::String),
        (parse_quote!(TrimMode), union!("start" | "end" | "both")),
        (parse_quote!(slumber_template::Value), TypeDef::Value),
        (parse_quote!(serde_json::Value), TypeDef::Value),
        (parse_quote!(Vec<String>), TypeDef::List(&TypeDef::String)),
    ])
}
