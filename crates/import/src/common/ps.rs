//! Generate PetitScript code from a collection

use crate::common::{
    Authentication, Chain, ChainId, ChainOutputTrim, ChainRequestSection,
    ChainRequestTrigger, ChainSource, Collection, Folder, Profile, Recipe,
    RecipeBody, RecipeNode, RecipeTree, SelectOptions, Template,
    template::{TemplateInputChunk, TemplateKey},
};
use indexmap::IndexMap;
use itertools::Itertools;
use petitscript::ast::{
    ArrayLiteral, AstVisitor, Declaration, Expression, FunctionBody,
    FunctionCall, FunctionDeclaration, FunctionDefinition, Identifier,
    ImportDeclaration, IntoExpression, IntoNode, IntoStatement, Literal,
    Module, Node, ObjectLiteral, Statement, TemplateChunk, TemplateLiteral,
    Walk,
};
use slumber_core::collection::RecipeId;
use std::sync::Arc;

const CHAIN_FN_PREFIX: &str = "chain_";

// TODO add note about function namespacing assumptions: slumber fns will never
// be shadowed

/// Define how a type should be converted into a PetitScript AST
trait IntoPetitAst {
    /// The AST type generated from this value. This should be as narrow as
    /// possible to make the returned node as flexible as possible. Also having
    /// a simple rule prevents decision making.
    type Output;

    fn into_ast(self) -> Self::Output;
}

impl Collection {
    /// Convert this collection into a PetitScript program
    pub fn into_petitscript(self) -> Module {
        self.into_ast()
    }
}

impl IntoPetitAst for Collection {
    type Output = Module;

    fn into_ast(self) -> Self::Output {
        let mut statements: Vec<Node<Statement>> = Vec::new();

        // For each chain, define a function
        statements.extend(
            self.chains
                .into_values()
                .map(|chain| chain.into_ast().into_stmt().s()),
        );

        // Generate an exported object literal for both profiles and recipes
        statements.push(
            Declaration::lexical("profiles", self.profiles.into_ast().into())
                .export()
                .s(),
        );
        statements.push(
            Declaration::lexical("requests", self.recipes.into_ast().into())
                .export()
                .s(),
        );

        // Walk through the generated AST and track any functions that were
        // called. Anything that doesn't start with "chain_" is a slumber fn
        let used_functions = find_slumber_functions(&mut statements);
        if !used_functions.is_empty() {
            // Yes inserting to the beginning of a vec is "slow" but it's only
            // once, this thing is never going to be *that* big, and this is in
            // human time
            statements.insert(
                0,
                ImportDeclaration::native(
                    None::<Identifier>,
                    used_functions,
                    "slumber",
                )
                .into_stmt()
                .s(),
            );
        }

        Module {
            statements: statements.into(),
        }
    }
}

impl IntoPetitAst for Chain {
    type Output = FunctionDeclaration;

    /// Generate a statement that declares a function. The function's execution
    /// will be equivalent to evaluating this chain.
    fn into_ast(self) -> Self::Output {
        /// Build an arg list of `R` required arguments plus one keyword
        /// argument of `O` entries. Any empty kwargs will be omitted.
        /// If all kwargs are empty, omit the entire kwargs object
        fn with_kwargs<const R: usize, const KW: usize>(
            required: [Expression; R],
            kwargs: [(&str, Option<Expression>); KW],
        ) -> Vec<Expression> {
            let mut arguments: Vec<Expression> = required.into();
            let kwargs = kwargs
                .into_iter()
                .filter_map(|(k, v)| Some((k, v?)))
                .collect_vec();
            if !kwargs.is_empty() {
                arguments.push(ObjectLiteral::new(kwargs).into());
            }
            arguments
        }

        // Populate the function body according to the source. Start with a
        // single function call
        let body_expression = match self.source {
            ChainSource::Command { command, stdin } => {
                let arguments = with_kwargs(
                    [command.into_ast().into()],
                    [("stdin", stdin.map(Template::into_ast))],
                );
                FunctionCall::named("command", arguments)
            }
            ChainSource::Environment { variable } => {
                FunctionCall::named("env", [variable.into_ast()])
            }
            ChainSource::File { path } => {
                FunctionCall::named("file", [path.into_ast()])
            }
            ChainSource::Prompt { message, default } => {
                let arguments = with_kwargs(
                    [],
                    [
                        ("message", message.map(Template::into_ast)),
                        ("default", default.map(Template::into_ast)),
                    ],
                );
                FunctionCall::named("prompt", arguments)
            }
            ChainSource::Request {
                recipe,
                trigger,
                section: ChainRequestSection::Body,
            } => {
                let arguments = with_kwargs(
                    [recipe.into_ast()],
                    [("trigger", trigger.into_ast())],
                );
                FunctionCall::named("response", arguments)
            }
            ChainSource::Request {
                recipe,
                trigger,
                section: ChainRequestSection::Header(header),
            } => {
                let arguments = with_kwargs(
                    [recipe.into_ast(), header.into_ast()],
                    [("trigger", trigger.into_ast())],
                );
                FunctionCall::named("responseHeader", arguments)
            }
            ChainSource::Select { message, options } => {
                let arguments = with_kwargs(
                    [options.into_ast()],
                    [("message", message.map(Template::into_ast))],
                );
                FunctionCall::named("select", arguments)
            }
        }
        .into_expr();

        // To replicate trimming, call the appropriate method from string's
        // prototype. This requires the expression to resolve to a string.
        let body_expression = match self.trim {
            ChainOutputTrim::None => body_expression,
            ChainOutputTrim::Start => body_expression.call("trimStart", []),
            ChainOutputTrim::End => body_expression.call("trimEnd", []),
            ChainOutputTrim::Both => body_expression.call("trim", []),
        };

        // TODO figure out how to do sensitive values
        // TODO implement selector, selector_mode, and content_type

        FunctionDefinition::new(
            // Chains don't accept params, so the function won't either
            [],
            FunctionBody::expression(body_expression),
        )
        .declare(chain_id_to_function(&self.id))
    }
}

impl IntoPetitAst for SelectOptions {
    type Output = Expression;

    /// Convert a static list of options into an array literal, or a dynamic
    /// template into an expression that will evaluate to an array
    fn into_ast(self) -> Self::Output {
        match self {
            SelectOptions::Fixed(templates) => {
                ArrayLiteral::new(templates.into_iter().map(Template::into_ast))
                    .into()
            }
            SelectOptions::Dynamic(template) => template.into_ast(),
        }
    }
}

impl IntoPetitAst for Profile {
    type Output = ObjectLiteral;

    /// Generate an object literal representing a profile
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::new([
            ("name", self.name.into()),
            ("default", self.default.into()),
            ("data", Deferred(self.data).into_ast().into()),
        ])
    }
}

impl IntoPetitAst for RecipeTree {
    type Output = ObjectLiteral;

    /// Recursively generate an object literal representing an entire recipe
    /// tree
    fn into_ast(self) -> Self::Output {
        self.tree.into_ast()
    }
}

impl IntoPetitAst for RecipeNode {
    type Output = ObjectLiteral;

    /// Generate an object literal representing a recipe/folder
    fn into_ast(self) -> Self::Output {
        match self {
            Self::Folder(folder) => folder.into_ast(),
            Self::Recipe(recipe) => recipe.into_ast(),
        }
    }
}

impl IntoPetitAst for Folder {
    type Output = ObjectLiteral;

    fn into_ast(self) -> Self::Output {
        ObjectLiteral::filtered([
            ("type", Some("folder".into())),
            ("name", self.name.map(Expression::from)),
            ("requests", Some(self.children.into_ast().into())),
        ])
    }
}

impl IntoPetitAst for RecipeId {
    type Output = Expression;

    fn into_ast(self) -> Self::Output {
        self.to_string().into()
    }
}

impl IntoPetitAst for Recipe {
    type Output = ObjectLiteral;

    /// Generate an object literal representing a recipe
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::filtered([
            ("type", Some("request".into())),
            ("name", self.name.map(Expression::from)),
            ("persist", Some(self.persist.into())),
            ("method", Some(self.method.to_str().into())),
            ("url", Some(Deferred(self.url).into_ast())),
            (
                "query",
                if self.query.is_empty() {
                    None
                } else {
                    Some(QueryParameters(self.query).into_ast().into())
                },
            ),
            (
                "headers",
                if self.headers.is_empty() {
                    None
                } else {
                    Some(Deferred(self.headers).into_ast().into())
                },
            ),
            (
                "authentication",
                self.authentication
                    .map(|authentication| authentication.into_ast().into()),
            ),
            ("body", self.body.map(RecipeBody::into_ast)),
        ])
    }
}

/// Newtype for converting query params
struct QueryParameters(Vec<(String, Template)>);

impl IntoPetitAst for QueryParameters {
    type Output = ObjectLiteral;

    /// The query parameter format changed to be:
    /// `{<param>: (<value> | [<value>, ...])}`
    /// So group by param
    fn into_ast(self) -> Self::Output {
        let grouped: IndexMap<String, Vec<Template>> = self.0.into_iter().fold(
            IndexMap::default(),
            |mut acc, (param, value)| {
                acc.entry(param).or_default().push(value);
                acc
            },
        );
        ObjectLiteral::new(grouped.into_iter().map(|(param, mut values)| {
            // If a param only has one value, flatten the vec
            let value = if values.len() == 1 {
                Deferred(values.remove(0)).into_ast()
            } else {
                ArrayLiteral::new(
                    values
                        .into_iter()
                        .map(|template| Deferred(template).into_ast()),
                )
                .into()
            };
            (param, value)
        }))
    }
}

impl IntoPetitAst for Authentication {
    type Output = ObjectLiteral;

    fn into_ast(self) -> Self::Output {
        match self {
            Self::Basic { username, password } => ObjectLiteral::filtered([
                ("type", Some("basic".into())),
                ("username", Some(Deferred(username).into_ast())),
                (
                    "password",
                    password.map(|password| Deferred(password).into_ast()),
                ),
            ]),
            Self::Bearer(token) => ObjectLiteral::new([
                ("type", "bearer".into()),
                ("token", Deferred(token).into_ast()),
            ]),
        }
    }
}

impl IntoPetitAst for RecipeBody {
    type Output = Expression;

    /// Convert a raw body to a string/template literal. Any other body will
    /// become an object literal
    fn into_ast(self) -> Self::Output {
        match self {
            // Raw string body -> create a string or template
            Self::Raw(body) => Deferred(body).into_ast(),
            Self::Json(json) => ObjectLiteral::new([
                ("type", "json".into()),
                // Convert the JSON into an equivalent expression. This will
                // parse templates within the JSON as needed
                ("data", Deferred(json).into_ast()),
            ])
            .into(),
            Self::FormUrlencoded(fields) => ObjectLiteral::new([
                ("type", "formUrlencoded".into()),
                ("data", Deferred(fields).into_ast().into()),
            ])
            .into(),
            Self::FormMultipart(fields) => ObjectLiteral::new([
                ("type", "formMultipart".into()),
                ("data", Deferred(fields).into_ast().into()),
            ])
            .into(),
        }
    }
}

impl IntoPetitAst for ChainRequestTrigger {
    type Output = Option<Expression>;

    /// Generate
    fn into_ast(self) -> Self::Output {
        match self {
            // The kwargs should be excluded if it's the default
            Self::Never => None,
            Self::NoHistory => Some("noHistory".into()),
            Self::Expire(duration) => Some(
                ObjectLiteral::new([
                    ("type", "expire".into()),
                    ("duration", "TODO format duration as string".into()),
                ])
                .into(),
            ),
            Self::Always => Some("always".into()),
        }
    }
}

impl IntoPetitAst for Template {
    type Output = Expression;

    /// Convert a legacy Slumber template to an expression. Empty and
    /// single-chunk templates will either become a string literal or a bare
    /// expression. Multi-chunk templates will be converted to a PS template
    /// literal.
    fn into_ast(self) -> Self::Output {
        match self.chunks.as_slice() {
            [] => "".into(),
            [TemplateInputChunk::Raw(s)] => s.as_str().into(),
            // Parent is responsible for deferring dynamic templates into a
            // lambda as needed. This is only necessary for top-level dynamic
            // templates so we don't want to do it all the time
            [TemplateInputChunk::Key(key)] => key.clone().into_ast().into(),
            _ => TemplateLiteral {
                // Convert each chunk and join them together
                chunks: self
                    .chunks
                    .into_iter()
                    .map(|chunk| {
                        match chunk {
                            TemplateInputChunk::Raw(s) => {
                                TemplateChunk::Literal(Arc::unwrap_or_clone(s))
                            }
                            TemplateInputChunk::Key(key) => {
                                TemplateChunk::Expression(
                                    key.into_ast().into_expr().s(),
                                )
                            }
                        }
                        .s()
                    })
                    .collect::<Vec<_>>()
                    .into(),
            }
            .into(),
        }
    }
}

impl IntoPetitAst for TemplateKey {
    type Output = FunctionCall;

    /// Generate an expression corresponding to a dynamic template key
    fn into_ast(self) -> Self::Output {
        match self {
            // `{{field1}}` -> `profile('field1')`
            TemplateKey::Field(identifier) => FunctionCall::named(
                "profile",
                [identifier.to_string().into_expr()],
            ),
            // `{{chains.chain1}}` -> `chain_chain1()`
            // Chain functions are always nullary because old chain references
            // had no way of passing arguments
            TemplateKey::Chain(chain_id) => {
                FunctionCall::named(chain_id_to_function(&chain_id), [])
            }
            // `{{env.VAR1}}` -> `env('VAR1')`
            TemplateKey::Environment(identifier) => {
                FunctionCall::named("env", [identifier.to_string().into_expr()])
            }
        }
    }
}

impl<T, E> IntoPetitAst for Vec<T>
where
    T: IntoPetitAst<Output = E>,
    Expression: From<E>,
{
    type Output = ArrayLiteral;

    /// Convert a vec into an array literal, mapping each value as we go
    fn into_ast(self) -> Self::Output {
        ArrayLiteral::new(self.into_iter().map(|e| e.into_ast().into()))
    }
}

impl<K, V, E> IntoPetitAst for IndexMap<K, V>
where
    K: Into<String>,
    V: IntoPetitAst<Output = E>,
    Expression: From<E>,
{
    type Output = ObjectLiteral;

    /// Convert a map into an object literal, mapping each value as we go
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::new(
            self.into_iter()
                .map(|(k, v)| (k.into(), v.into_ast().into())),
        )
    }
}

/// A newtype to indicate a template's resolution should be deferred via a
/// nullary lambda. I.e. convert `template` to `() => template`. This should be
/// used on any top-level template (in recipes and profiles), but not on
/// templates nested within chain bodies. This is necessary because YAML
/// templates are deferred by default, and the render engine would implicitly
/// render nested templates.
struct Deferred<T>(T);

impl IntoPetitAst for Deferred<Template> {
    type Output = Expression;

    /// Defer dynamic templates into a function. Static templates convert to
    /// literals and don't need to be deferred
    fn into_ast(self) -> Self::Output {
        match self.0.into_ast() {
            // A literal doesn't need to be deferred
            expression @ Expression::Literal(_) => expression,
            expression => FunctionDefinition::new(
                [],
                FunctionBody::expression(expression),
            )
            .into(),
        }
    }
}

impl IntoPetitAst for Deferred<serde_json::Value> {
    type Output = Expression;

    /// Convert a JSON value to a literal expression, and defer it if it
    /// contains any nested templates. This will parse every string in the JSON
    /// as a template and it any of them contain dynamic chunks, the whole
    /// object will be deferred at the top level.
    fn into_ast(self) -> Self::Output {
        /// Recursively convert a value, and enable the given flag the first
        /// time we hit a dynamic template
        fn convert(
            value: serde_json::Value,
            is_dynamic: &mut bool,
        ) -> Expression {
            match value {
                serde_json::Value::Null => {
                    Expression::Literal(Literal::Null.s())
                }
                serde_json::Value::Bool(b) => b.into(),
                serde_json::Value::Number(number) => todo!(),
                serde_json::Value::String(s) => convert_string(s, is_dynamic),
                serde_json::Value::Array(array) => ArrayLiteral::new(
                    array
                        .into_iter()
                        .map(|element| convert(element, is_dynamic)),
                )
                .into(),
                serde_json::Value::Object(map) => {
                    ObjectLiteral::new(map.into_iter().map(|(k, v)| {
                        // We have to support templates in both keys and values
                        let key = convert_string(k, is_dynamic);
                        let value = convert(v, is_dynamic);
                        (key, value)
                    }))
                    .into()
                }
            }
        }

        /// Convert a string to an expression. If it's a dynamic template,
        /// enable the flag
        fn convert_string(s: String, is_dynamic: &mut bool) -> Expression {
            // Theoretically the string should be a valid template, but if not
            // treat it literally
            match s.parse::<Template>() {
                Ok(template) => {
                    *is_dynamic |= template.is_dynamic();
                    template.into_ast()
                }
                Err(_) => s.into(),
            }
        }

        let mut is_dynamic = false;
        let expression = convert(self.0, &mut is_dynamic);
        // If the JSON contained any templates, it's dynamic so we need to
        // defer it
        if is_dynamic {
            FunctionDefinition::new([], FunctionBody::expression(expression))
                .into()
        } else {
            expression
        }
    }
}

impl IntoPetitAst for Deferred<IndexMap<String, Template>> {
    type Output = ObjectLiteral;

    /// Defer the evaluation of each template in the map
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::new(
            self.0.into_iter().map(|(k, v)| (k, Deferred(v).into_ast())),
        )
    }
}

/// Get a function name from a chain ID
fn chain_id_to_function(chain_id: &ChainId) -> Identifier {
    // TODO normalize ID to make sure it's a valid fn name
    Identifier::new(format!("{CHAIN_FN_PREFIX}{}", chain_id.0))
}

/// Find all called functions that aren't chain functions. These functions need
/// to be imported from the `slumber` module. All included functions should be
/// available in the module; if not, we fucked up somewhere. The returned vec
/// will be de-duplicated.
fn find_slumber_functions(
    statements: &mut [Node<Statement>],
) -> Vec<Identifier> {
    struct Visitor(Vec<Identifier>);

    impl AstVisitor for Visitor {
        fn enter_function_call(&mut self, function_call: &mut FunctionCall) {
            // unstable: if-let chain
            // https://github.com/rust-lang/rust/pull/132833
            match &**function_call.function {
                Expression::Identifier(identifier)
                    if !identifier.as_str().starts_with(CHAIN_FN_PREFIX) =>
                {
                    self.0.push(identifier.data().clone());
                }
                _ => {}
            }
        }
    }

    let mut visitor = Visitor(Vec::new());
    for statement in statements {
        statement.walk(&mut visitor);
    }
    // Maybe we should just use a hashset? This ensures the ordering is
    // deterministic though
    let mut names = visitor.0;
    names.sort();
    names.dedup();
    names
}

// TODO test the shit out of this
