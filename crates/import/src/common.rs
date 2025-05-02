//! Generate PetitScript code from a collection

use crate::ImportCollection;
use indexmap::IndexMap;
use itertools::Itertools;
use petitscript::{
    Value,
    ast::{
        ArrayLiteral, AstVisitor, Declaration, Expression, FunctionBody,
        FunctionCall, FunctionDefinition, Identifier, ImportDeclaration,
        IntoNode, IntoStatement, Module, ObjectLiteral, Statement, Walk,
    },
};
use slumber_core::{
    collection::{
        Authentication, Folder, Profile, QueryParameterValue, Recipe,
        RecipeBody, RecipeId, RecipeNode, RecipeTree,
    },
    ps,
};
use std::collections::HashSet;

// TODO remove words "template" and "procedure" everywhere

impl ImportCollection {
    /// Generate a PetitScript AST for this collection. The AST can then be
    /// converted into source code
    pub fn into_petitscript(self) -> Module {
        self.into_ast()
    }
}

/// Generate a function call expression for a native function by name. Pass `R`
/// required arguments plus one keyword argument of `KW` entries. Any empty
/// kwargs will be omitted. If all kwargs are empty, omit the entire kwargs
/// object.
///
/// TODO it would be sick to leverage the typing of the actual functions
/// somehow. Maybe a macro?
pub fn call_fn<const R: usize, const KW: usize>(
    name: &'static str,
    required: [Expression; R],
    kwargs: [(&str, Option<Expression>); KW],
) -> FunctionCall {
    let mut arguments: Vec<Expression> = required.into();
    let kwargs = kwargs
        .into_iter()
        .filter_map(|(k, v)| Some((k, v?)))
        .collect_vec();
    if !kwargs.is_empty() {
        arguments.push(ObjectLiteral::new(kwargs).into());
    }
    FunctionCall::named(name, arguments)
}

/// Convert this type into a PetitScript AST element
pub trait IntoPetitAst {
    /// The AST type generated from this value. This should be as narrow as
    /// possible to make the returned node as flexible as possible. Also having
    /// a simple rule prevents decision making.
    type Output;

    fn into_ast(self) -> Self::Output;
}

impl IntoPetitAst for ImportCollection {
    type Output = Module;

    fn into_ast(self) -> Self::Output {
        let mut statements: Vec<Statement> = Vec::new();

        statements.extend(
            self.declarations
                .into_iter()
                .map(|declaration| Statement::Declaration(declaration.s())),
        );

        // Generate an exported object literal for both profiles and recipes
        statements.push(
            Declaration::new("profiles", self.profiles.into_ast().into())
                .export(),
        );
        statements.push(
            Declaration::new("requests", self.recipes.into_ast().into())
                .export(),
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
                .into_stmt(),
            );
        }

        Module::new(statements)
    }
}

impl IntoPetitAst for Profile<Expression> {
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

impl IntoPetitAst for RecipeTree<Expression> {
    type Output = ObjectLiteral;

    /// Recursively generate an object literal representing an entire recipe
    /// tree
    fn into_ast(self) -> Self::Output {
        self.into_map().into_ast()
    }
}

impl IntoPetitAst for RecipeNode<Expression> {
    type Output = ObjectLiteral;

    /// Generate an object literal representing a recipe/folder
    fn into_ast(self) -> Self::Output {
        match self {
            Self::Folder(folder) => folder.into_ast(),
            Self::Recipe(recipe) => recipe.into_ast(),
        }
    }
}

impl IntoPetitAst for Folder<Expression> {
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

impl IntoPetitAst for Recipe<Expression> {
    type Output = ObjectLiteral;

    /// Generate an object literal representing a recipe
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::filtered([
            ("type", Some("request".into())),
            ("name", self.name.map(Expression::from)),
            // Only include this field if it's not the default
            ("persist", (!self.persist).then_some(false.into())),
            ("method", Some(self.method.to_string().into())),
            ("url", Some(Deferred(self.url).into_ast())),
            (
                "query",
                if self.query.is_empty() {
                    None
                } else {
                    Some(Deferred(self.query).into_ast().into())
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

impl IntoPetitAst for Authentication<Expression> {
    type Output = ObjectLiteral;

    fn into_ast(self) -> Self::Output {
        match self {
            Self::Basic { username, password } => ObjectLiteral::new([
                ("type", "basic".into()),
                ("username", Deferred(username).into_ast()),
                ("password", Deferred(password).into_ast()),
            ]),
            Self::Bearer { token } => ObjectLiteral::new([
                ("type", "bearer".into()),
                ("token", Deferred(token).into_ast()),
            ]),
        }
    }
}

impl IntoPetitAst for RecipeBody<Expression> {
    type Output = Expression;

    /// Convert a raw body to a string/template literal. Any other body will
    /// become an object literal
    fn into_ast(self) -> Self::Output {
        match self {
            // Raw string body -> create a string or template
            Self::Raw { data } => Deferred(data).into_ast(),
            Self::Json { data } => ObjectLiteral::new([
                ("type", "json".into()),
                ("data", Deferred(data).into_ast()),
            ])
            .into(),
            Self::FormUrlencoded { data } => ObjectLiteral::new([
                ("type", "formUrlencoded".into()),
                ("data", Deferred(data).into_ast().into()),
            ])
            .into(),
            Self::FormMultipart { data } => ObjectLiteral::new([
                ("type", "formMultipart".into()),
                ("data", Deferred(data).into_ast().into()),
            ])
            .into(),
        }
    }
}

impl<E, T> IntoPetitAst for Vec<T>
where
    T: IntoPetitAst<Output = E>,
    Expression: From<E>,
{
    type Output = ArrayLiteral;

    fn into_ast(self) -> Self::Output {
        ArrayLiteral::new(self.into_iter().map(|value| value.into_ast().into()))
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

/// Enable blanket impls for Vec<Expression>
impl IntoPetitAst for Expression {
    type Output = Expression;

    fn into_ast(self) -> Self::Output {
        self
    }
}

/// A newtype to indicate a template's resolution should be deferred via a
/// nullary lambda. I.e. convert `template` to `() => template`. This should be
/// used on any top-level template (in recipes and profiles), but not on
/// templates nested within chain bodies. This is necessary because YAML
/// templates are deferred by default, and the render engine would implicitly
/// render nested templates.
///
/// TODO would this be better as a trait? call <stuff>.deferred()
struct Deferred<T>(T);

impl IntoPetitAst for Deferred<Expression> {
    type Output = Expression;

    /// Defer dynamic expressions into a function. Literal expressions don't
    /// need to be deferred
    fn into_ast(mut self) -> Self::Output {
        // This will recursively scan the expression for anything that's not a
        // literal
        if is_dynamic(&mut self.0) {
            FunctionDefinition::new([], FunctionBody::expression(self.0)).into()
        } else {
            self.0
        }
    }
}

/// Query parameters are only used at the top level, so their conversions will
/// always be deferred
impl IntoPetitAst for Deferred<QueryParameterValue<Expression>> {
    type Output = Expression;

    fn into_ast(self) -> Self::Output {
        match self.0 {
            QueryParameterValue::Single(procedure) => {
                // Defer to Deferred!
                Deferred(procedure).into_ast()
            }
            QueryParameterValue::Many(mut expressions) => {
                // If _any_ expression is dynamic, we need to defer the whole
                // array
                if expressions.iter_mut().any(is_dynamic) {
                    FunctionDefinition::new(
                        [],
                        FunctionBody::expression(expressions.into_ast().into()),
                    )
                    .into()
                } else {
                    // Use a plain array literal
                    expressions.into_ast().into()
                }
            }
        }
    }
}

impl<K, V, E> IntoPetitAst for Deferred<IndexMap<K, V>>
where
    K: Into<String>,
    Deferred<V>: IntoPetitAst<Output = E>,
    Expression: From<E>,
{
    type Output = ObjectLiteral;

    /// Defer the evaluation of each template in the map
    fn into_ast(self) -> Self::Output {
        ObjectLiteral::new(
            self.0
                .into_iter()
                .map(|(k, v)| (k.into(), Deferred(v).into_ast().into())),
        )
    }
}

/// Find all called functions from the `slumber` module. The returned vec will
/// be de-duplicated.
fn find_slumber_functions(statements: &mut [Statement]) -> Vec<Identifier> {
    struct Visitor<'a> {
        /// All functions in the slumber module
        slumber_fns: &'a IndexMap<String, Value>,
        to_import: HashSet<Identifier>,
    }

    impl AstVisitor for Visitor<'_> {
        fn enter_function_call(&mut self, function_call: &mut FunctionCall) {
            // unstable: if-let chain
            // https://github.com/rust-lang/rust/pull/132833
            match &**function_call.function {
                Expression::Identifier(identifier)
                    if self.slumber_fns.contains_key(identifier.as_str()) =>
                {
                    self.to_import.insert(identifier.data().clone());
                }
                _ => {}
            }
        }
    }

    let mut visitor = Visitor {
        // Building the whole module just to get a list of fn names is a bit
        // clumsy, but the cost is negligible
        slumber_fns: &ps::module().named,
        to_import: HashSet::new(),
    };
    for statement in statements {
        statement.walk(&mut visitor);
    }

    // Sort alphabetically to get a determinisitic ordering
    visitor.to_import.into_iter().sorted().collect()
}

/// Check if the expression is a static literal or contains any dynamic aspects.
/// This will recursively check array/object literals to ensure all values are
/// static as well.
///
/// The expression won't be mutated; `&mut` is just needed for the AST walker
fn is_dynamic(expression: &mut Expression) -> bool {
    struct Visitor {
        is_dynamic: bool,
    }

    impl AstVisitor for Visitor {
        fn enter_expression(&mut self, expression: &mut Expression) {
            if !matches!(expression, Expression::Literal(_)) {
                self.is_dynamic = true;
            }
        }
    }

    let mut visitor = Visitor { is_dynamic: false };
    expression.walk(&mut visitor);
    visitor.is_dynamic
}

// TODO test some select edge cases. the common cases will be covered by
// integration tests
