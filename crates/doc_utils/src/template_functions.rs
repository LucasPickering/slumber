//! Generate documentation for template functions.

use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use itertools::Itertools;
use quote::ToTokens;
use serde::Deserialize;
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
/// Rust file containing template function definitions. Relative to docs/
const INPUT_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../core/src/render/functions.rs",
);

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
        .context(format!("Error opening {INPUT_FILE}"))?;

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
#[derive(Clone, Debug)]
struct TemplateFunctionMetadata {
    name: String,
    /// Documentation is YAML embedded in the doc comment. We enforce the
    /// format of the YAML to make sure every function has proper
    /// documentation
    documentation: Documentation,
    parameters: Vec<ParameterMetadata>,
    return_type: TypeDef,
}

impl TemplateFunctionMetadata {
    /// Load function metadata from the given definition
    fn from_item_fn(func: ItemFn) -> Result<Self> {
        let name = func.sig.ident.to_string();
        let parameters = Self::get_parameters(&func.sig.inputs)?;
        let return_type = Self::get_return_type(&func.sig.output)?;
        let doc_comment = Self::get_doc_comment(&func)?;
        let documentation =
            Documentation::parse(&doc_comment, &parameters, return_type)
                .context(format!(
                    "Error parsing doc comment for function `{name}`"
                ))?;

        Ok(TemplateFunctionMetadata {
            name,
            documentation,
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
                    Some(lit_str.value())
                } else {
                    None
                }
            })
            .join("\n");

        if doc.is_empty() {
            return Err(anyhow!(
                "function `{}` is missing a doc comment",
                func.sig.ident
            ));
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
        fn fmt_parameters(parameters: &[ParameterMetadata]) -> String {
            // If the params get too long, overflow onto multiple lines
            let one_line = parameters.iter().join(", ").to_string();
            if one_line.len() <= 60 {
                one_line
            } else {
                format!(
                    "\n{}",
                    parameters.iter().format_with("", |param, f| f(
                        &format_args!("  {param},\n")
                    ))
                )
            }
        }

        write!(
            f,
            "### {name}

```typescript
{name}({parameters}): {return_type}
```

{doc}",
            name = self.name,
            parameters = fmt_parameters(&self.parameters),
            return_type = self.return_type,
            doc = self.documentation,
        )
    }
}

/// Information about a Rust function parameter
#[derive(Clone, Debug)]
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

/// Doc comments are written as YAML and deserialized into this format
#[derive(Clone, Debug, Deserialize)]
struct Documentation {
    description: String,
    parameters: IndexMap<String, DocParameter>,
    #[serde(rename = "return")]
    return_description: String,
    /// Populated post-parse with the function signature data
    #[serde(skip)]
    return_type: Option<TypeDef>,
    examples: Vec<DocExample>,
    #[serde(default)]
    errors: Vec<String>,
}

impl Documentation {
    /// Parse the doc comment as YAML. Supplement the parsed data with types
    /// from the function definition
    fn parse(
        comment: &str,
        parameters: &[ParameterMetadata],
        return_type: TypeDef,
    ) -> Result<Self> {
        // Trim whitespace and backticks. Backticks are allowed to disable
        // formatting within the YAML
        let comment = comment
            .trim()
            .strip_prefix("```notrust")
            .expect("Doc comment should start with ```notrust")
            .strip_suffix("```")
            .expect("Doc comment should end with ```");
        let mut documentation: Self = serde_yaml::from_str(comment)?;

        // Supplementation
        documentation.return_type = Some(return_type);
        // Add type data to each param
        for parameter in parameters {
            let name = &parameter.name;
            let param_doc =
                documentation.parameters.get_mut(name).ok_or_else(|| {
                    anyhow!("Missing documentation for parameter `{name}`")
                })?;
            param_doc.type_def = Some(parameter.type_def);
            // Make sure `default` field is provided only when appropriate
            match (parameter.is_kwarg, &param_doc.default) {
                (false, Some(_)) => bail!(
                    "required argument `{name}` should not have `default` value"
                ),
                (true, None) => bail!("kwarg `{name}` missing `default` value"),
                _ => {}
            }
        }
        // Make sure every param in the doc is also in the type signature
        for (name, parameter) in &documentation.parameters {
            if parameter.type_def.is_none() {
                bail!(
                    "argument `{name}` is documented but not in the \
                    function type signature"
                );
            }
        }

        // Safety checks
        if documentation.examples.is_empty() {
            bail!("Missing examples")
        }

        Ok(documentation)
    }
}

impl Display for Documentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn write_title(f: &mut fmt::Formatter<'_>, title: &str) -> fmt::Result {
            writeln!(f, "**{title}**\n")
        }

        writeln!(f, "{}", self.description)?;
        writeln!(f)?;

        // Parameters
        write_title(f, "Parameters")?;
        if self.parameters.is_empty() {
            writeln!(f, "None")?;
        } else {
            for (name, parameter) in &self.parameters {
                let req = if let Some(default) = &parameter.default {
                    format!("default = `{default}`")
                } else {
                    "required".into()
                };
                writeln!(
                    f,
                    "- `{name}: {type_def}` ({req}): {description}",
                    type_def = parameter.type_def.unwrap(),
                    description = parameter.description,
                )?;
            }
        }
        writeln!(f)?;

        // Return value
        write_title(f, "Return")?;
        writeln!(f, "`{}`", self.return_type.unwrap())?;
        writeln!(f)?;
        writeln!(f, "{}", self.return_description)?;
        writeln!(f)?;

        // Errors
        if !self.errors.is_empty() {
            write_title(f, "Errors")?;
            for error in &self.errors {
                writeln!(f, "- {error}")?;
            }
            writeln!(f)?;
        }

        // Examples. Parsing ensures this is not empty
        write_title(f, "Examples")?;
        writeln!(f, "```sh")?; // Use sh for reasonable syntax highlighting
        for example in &self.examples {
            if let Some(comment) = &example.comment {
                writeln!(f, "# {comment}")?;
            }
            writeln!(
                f,
                "{input} => {output}",
                input = example.input,
                output = example.output
            )?;
        }
        writeln!(f, "```")?;

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DocParameter {
    description: String,
    default: Option<String>,
    /// Populated post-parse with the function signature data
    #[serde(skip)]
    type_def: Option<TypeDef>,
}

#[derive(Clone, Debug, Deserialize)]
struct DocExample {
    input: String,
    output: String,
    comment: Option<String>,
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
    Float,
    Integer,
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
    /// An object with a static set of fields
    Struct(&'static [(&'static str, &'static Self)]),
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

        TYPE_MAP.with(|map| map.get(ty).copied()).ok_or_else(|| {
            anyhow!(
                "Unmapped type {}; add the type to type_map() in {}",
                ty.to_token_stream(),
                file!()
            )
        })
    }
}

impl Display for TypeDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boolean => write!(f, "boolean"),
            Self::Float => write!(f, "float"),
            Self::Integer => write!(f, "integer"),
            Self::Bytes => write!(f, "bytes"),
            Self::String => write!(f, "string"),
            Self::Literal(literal) => write!(f, "\"{literal}\""),
            Self::Value => write!(f, "value"),
            Self::List(inner) => {
                // Wrap unions in () to make the grouping clear
                if let Self::Union(_) = inner {
                    write!(f, "({inner})[]")
                } else {
                    write!(f, "{inner}[]")
                }
            }
            Self::Struct(fields) => {
                // { "field1": value1, "field2": value2 }
                write!(
                    f,
                    "{{ {} }}",
                    fields.iter().format_with(", ", |(field, value), f| f(
                        &format_args!("\"{field}\": {value}")
                    ))
                )
            }
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
        (parse_quote!(f64), TypeDef::Float),
        (parse_quote!(i64), TypeDef::Integer),
        (parse_quote!(Bytes), TypeDef::Bytes),
        (
            parse_quote!(CommandOutputMode),
            union!("stdout" | "stderr" | "both"),
        ),
        (parse_quote!(JaqQuery), TypeDef::String),
        (parse_quote!(JsonPath), TypeDef::String),
        (parse_quote!(JsonValue), TypeDef::Value),
        (
            parse_quote!(JsonQueryMode),
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
        (
            parse_quote!(Vec<SelectOption>),
            TypeDef::List(&TypeDef::Union(&[
                &TypeDef::String,
                &TypeDef::Struct(&[
                    ("label", &TypeDef::String),
                    ("value", &TypeDef::Value),
                ]),
            ])),
        ),
        (parse_quote!(String), TypeDef::String),
        // We're hiding streams from the type system, since they will
        // transparently convert to bytes
        (parse_quote!(LazyValue), TypeDef::Bytes),
        (parse_quote!(TrimMode), union!("start" | "end" | "both")),
        (parse_quote!(slumber_template::Value), TypeDef::Value),
        (parse_quote!(Value), TypeDef::Value),
        (parse_quote!(serde_json::Value), TypeDef::Value),
        (parse_quote!(Vec<String>), TypeDef::List(&TypeDef::String)),
    ])
}
