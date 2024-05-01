// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
mod tests;

use miette::Diagnostic;
use qsc_ast::{
    ast::{
        self, CallableBody, CallableDecl, Ident, Idents, NodeId, SpecBody, SpecGen, TopLevelNode,
    },
    visit::{self as ast_visit, walk_attr, Visitor as AstVisitor},
};

use qsc_data_structures::{
    index_map::IndexMap,
    namespaces::{NamespaceId, NamespaceTreeRoot, PRELUDE},
    span::Span,
};
use qsc_hir::{
    assigner::Assigner,
    global,
    hir::{self, ItemId, ItemStatus, LocalItemId, PackageId},
    ty::{ParamId, Prim},
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Ordering;
use std::{collections::hash_map::Entry, rc::Rc, str::FromStr, vec};
use thiserror::Error;

use crate::compile::preprocess::TrackedName;

// All AST Path nodes get mapped
// All AST Ident nodes get mapped, except those under AST Path nodes
pub(super) type Names = IndexMap<NodeId, Res>;

/// A resolution. This connects a usage of a name with the declaration of that name by uniquely
/// identifying the node that declared it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Res {
    /// A global or local item.
    Item(ItemId, ItemStatus),
    /// A local variable.
    Local(NodeId),
    /// A type/functor parameter in the generics section of the parent callable decl.
    Param(ParamId),
    /// A primitive type.
    PrimTy(Prim),
    /// The unit type.
    UnitTy,
}

#[derive(Clone, Debug, Diagnostic, Error)]
pub(super) enum Error {
    #[error("`{name}` could refer to the item in `{first_open}` or `{second_open}`")]
    #[diagnostic(code("Qsc.Resolve.Ambiguous"))]
    Ambiguous {
        name: String,
        first_open: String,
        second_open: String,
        #[label("ambiguous name")]
        name_span: Span,
        #[label("found in this namespace")]
        first_open_span: Span,
        #[label("and also in this namespace")]
        second_open_span: Span,
    },

    #[error("`{name}` could refer to the item in `{candidate_a}` or an item in `{candidate_b}`")]
    #[diagnostic(help("both namespaces are implicitly opened by the prelude"))]
    #[diagnostic(code("Qsc.Resolve.AmbiguousPrelude"))]
    AmbiguousPrelude {
        name: String,
        candidate_a: String,
        candidate_b: String,
        #[label("ambiguous name")]
        span: Span,
    },

    #[error("duplicate declaration of `{0}` in namespace `{1}`")]
    #[diagnostic(code("Qsc.Resolve.Duplicate"))]
    Duplicate(String, String, #[label] Span),

    #[error("duplicate name `{0}` in pattern")]
    #[diagnostic(help("a name cannot shadow another name in the same pattern"))]
    #[diagnostic(code("Qsc.Resolve.DuplicateBinding"))]
    DuplicateBinding(String, #[label] Span),

    #[error("duplicate intrinsic `{0}`")]
    #[diagnostic(help(
        "each callable declared as `body intrinsic` must have a globally unique name"
    ))]
    #[diagnostic(code("Qsc.Resolve.DuplicateIntrinsic"))]
    DuplicateIntrinsic(String, #[label] Span),

    #[error("`{0}` not found")]
    #[diagnostic(code("Qsc.Resolve.NotFound"))]
    NotFound(String, #[label] Span),

    #[error("`{0}` not found")]
    #[diagnostic(help(
        "found a matching item `{1}` that is not available for the current compilation configuration"
    ))]
    #[diagnostic(code("Qsc.Resolve.NotFound"))]
    NotAvailable(String, String, #[label] Span),

    #[error("use of unimplemented item `{0}`")]
    #[diagnostic(help("this item is not implemented and cannot be used"))]
    #[diagnostic(code("Qsc.Resolve.Unimplemented"))]
    Unimplemented(String, #[label] Span),
}

#[derive(Debug, Clone)]
pub struct Scope {
    /// The span that the scope applies to. For callables and namespaces, this includes
    /// the entire callable / namespace declaration. For blocks, this includes the braces.
    span: Span,
    kind: ScopeKind,
    /// Open statements. The key is the namespace name or alias.
    opens: FxHashMap<Vec<Rc<str>>, Vec<Open>>,
    /// Local newtype declarations.
    tys: FxHashMap<Rc<str>, ItemId>,
    /// Local callable and newtype declarations.
    terms: FxHashMap<Rc<str>, ItemId>,
    /// Local variables, including callable parameters, for loop bindings, etc.
    /// The u32 is the `valid_at` offset - the lowest offset at which the variable name is available.
    /// It's used to determine which variables are visible at a specific offset in the scope.
    ///
    /// Bug: Because we keep track of only one `valid_at` offset per name,
    /// when a variable is later shadowed in the same scope,
    /// it is missed in the list. https://github.com/microsoft/qsharp/issues/897
    vars: FxHashMap<Rc<str>, (u32, NodeId)>,
    /// Type parameters.
    ty_vars: FxHashMap<Rc<str>, ParamId>,
}

impl Scope {
    fn new(kind: ScopeKind, span: Span) -> Self {
        Self {
            span,
            kind,
            opens: FxHashMap::default(),
            tys: FxHashMap::default(),
            terms: FxHashMap::default(),
            vars: FxHashMap::default(),
            ty_vars: FxHashMap::default(),
        }
    }

    fn item(&self, kind: NameKind, name: &str) -> Option<&ItemId> {
        let items = match kind {
            NameKind::Ty => &self.tys,
            NameKind::Term => &self.terms,
        };
        items.get(name)
    }
}

type ScopeId = usize;

#[derive(Debug, Clone, Default)]
pub struct Locals {
    // order is ascending by span (outermost -> innermost)
    scopes: Vec<Scope>,
}

impl Locals {
    fn get_scopes<'a>(&'a self, scope_chain: &'a [ScopeId]) -> impl Iterator<Item = &Scope> + 'a {
        // reverse to go from innermost -> outermost
        scope_chain.iter().rev().map(|id| {
            self.scopes
                .get(*id)
                .unwrap_or_else(|| panic!("scope with id {id:?} should exist"))
        })
    }

    fn push_scope(&mut self, s: Scope) -> ScopeId {
        let id = self.scopes.len();
        self.scopes.insert(id, s);
        id
    }

    fn get_scope_mut(&mut self, id: ScopeId) -> &mut Scope {
        self.scopes
            .get_mut(id)
            .unwrap_or_else(|| panic!("scope with id {id:?} should exist"))
    }

    #[must_use]
    pub fn get_all_at_offset(&self, offset: u32) -> Vec<Local> {
        let mut vars = true;
        let mut all_locals = Vec::new();
        self.for_each_scope_at_offset(offset, |scope| {
            // inner to outer
            all_locals.extend(get_scope_locals(scope, offset, vars));

            if scope.kind == ScopeKind::Callable {
                // Since local callables are not closures, hide local variables in parent scopes.
                vars = false;
            }
        });

        // deduping by name will effectively make locals in a child scope
        // shadow the locals in its parent scopes
        all_locals.dedup_by(|a, b| a.name == b.name);

        all_locals
    }

    fn for_each_scope_at_offset<F>(&self, offset: u32, mut f: F)
    where
        F: FnMut(&Scope),
    {
        // reverse to go from innermost -> outermost
        self.scopes.iter().rev().for_each(|scope| {
            // the block span includes the delimiters (e.g. the braces)
            if scope.span.lo < offset && scope.span.hi > offset {
                f(scope);
            }
        });
    }
}

#[derive(Debug)]
pub struct Local {
    pub name: Rc<str>,
    pub kind: LocalKind,
}

#[derive(Debug)]
pub enum LocalKind {
    /// A local callable or UDT.
    Item(ItemId),
    /// A type parameter.
    TyParam(ParamId),
    /// A local variable or parameter.
    Var(NodeId),
}

#[derive(Debug, Clone, Default)]
pub struct GlobalScope {
    tys: FxHashMap<NamespaceId, FxHashMap<Rc<str>, Res>>,
    terms: FxHashMap<NamespaceId, FxHashMap<Rc<str>, Res>>,
    namespaces: NamespaceTreeRoot,
    intrinsics: FxHashSet<Rc<str>>,
}

impl GlobalScope {
    fn find_namespace<'a>(&self, ns: impl IntoIterator<Item = &'a str>) -> Option<NamespaceId> {
        self.namespaces.get_namespace_id(ns)
    }

    fn get(&self, kind: NameKind, namespace: NamespaceId, name: &str) -> Option<&Res> {
        let items = match kind {
            NameKind::Ty => &self.tys,
            NameKind::Term => &self.terms,
        };
        items.get(&namespace).and_then(|items| items.get(name))
    }

    /// Creates a namespace in the namespace mapping. Note that namespaces are tracked separately from their
    /// item contents. This returns a [`NamespaceId`] which you can use to add more tys and terms to the scope.
    fn insert_or_find_namespace(&mut self, name: impl Into<Vec<Rc<str>>>) -> NamespaceId {
        self.namespaces.insert_or_find_namespace(name.into())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ScopeKind {
    Namespace(NamespaceId),
    Callable,
    Block,
}

#[derive(Clone, Copy)]
enum NameKind {
    Ty,
    Term,
}

#[derive(Debug, Clone)]
struct Open {
    namespace: NamespaceId,
    span: Span,
}

impl Eq for Open {}

impl PartialEq<Self> for Open {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace
    }
}

impl PartialOrd<Self> for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let a: usize = self.namespace.into();
        let b: usize = other.namespace.into();
        a.cmp(&b)
    }
}

pub(super) struct Resolver {
    names: Names,
    dropped_names: Vec<TrackedName>,
    curr_params: Option<FxHashSet<Rc<str>>>,
    curr_scope_chain: Vec<ScopeId>,
    globals: GlobalScope,
    locals: Locals,
    errors: Vec<Error>,
}

impl Resolver {
    pub(super) fn new(globals: GlobalTable, dropped_names: Vec<TrackedName>) -> Self {
        Self {
            names: globals.names,
            dropped_names,
            curr_params: None,
            globals: globals.scope,
            locals: Locals::default(),
            curr_scope_chain: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub(super) fn with_persistent_local_scope(
        globals: GlobalTable,
        dropped_names: Vec<TrackedName>,
    ) -> Self {
        let mut locals = Locals::default();
        let scope_id = locals.push_scope(Scope::new(
            ScopeKind::Block,
            Span {
                lo: 0,
                hi: u32::MAX,
            },
        ));
        Self {
            names: globals.names,
            dropped_names,
            curr_params: None,
            globals: globals.scope,
            locals,
            curr_scope_chain: vec![scope_id],
            errors: Vec::new(),
        }
    }

    pub(super) fn names(&self) -> &Names {
        &self.names
    }

    pub(super) fn locals(&self) -> &Locals {
        &self.locals
    }

    pub(super) fn drain_errors(&mut self) -> vec::Drain<Error> {
        self.errors.drain(..)
    }

    pub(super) fn with<'a>(&'a mut self, assigner: &'a mut Assigner) -> With<'a> {
        With {
            resolver: self,
            assigner,
        }
    }

    pub(super) fn into_result(self) -> (Names, Locals, Vec<Error>, NamespaceTreeRoot) {
        (
            self.names,
            self.locals,
            self.errors,
            self.globals.namespaces,
        )
    }

    pub(super) fn extend_dropped_names(&mut self, dropped_names: Vec<TrackedName>) {
        self.dropped_names.extend(dropped_names);
    }

    pub(super) fn bind_fragments(&mut self, ast: &ast::Package, assigner: &mut Assigner) {
        for node in &mut ast.nodes.iter() {
            match node {
                TopLevelNode::Namespace(namespace) => {
                    bind_global_items(
                        &mut self.names,
                        &mut self.globals,
                        namespace,
                        assigner,
                        &mut self.errors,
                    );
                }
                TopLevelNode::Stmt(stmt) => {
                    if let ast::StmtKind::Item(item) = stmt.kind.as_ref() {
                        self.bind_local_item(assigner, item);
                    }
                }
            }
        }
    }

    fn check_item_status(&mut self, res: Res, name: String, span: Span) {
        if let Res::Item(_, ItemStatus::Unimplemented) = res {
            self.errors.push(Error::Unimplemented(name, span));
        }
    }

    fn resolve_ident(&mut self, kind: NameKind, name: &Ident) {
        let namespace = None;

        match resolve(
            kind,
            &self.globals,
            self.locals.get_scopes(&self.curr_scope_chain),
            name,
            &namespace,
        ) {
            Ok(res) => {
                self.check_item_status(res, name.name.to_string(), name.span);
                self.names.insert(name.id, res);
            }
            Err(err) => self.errors.push(err),
        }
    }

    fn resolve_path(&mut self, kind: NameKind, path: &ast::Path) {
        let name = &path.name;
        let namespace = &path.namespace;

        match resolve(
            kind,
            &self.globals,
            self.locals.get_scopes(&self.curr_scope_chain),
            name,
            namespace,
        ) {
            Ok(res) => {
                self.check_item_status(res, path.name.name.to_string(), path.span);
                self.names.insert(path.id, res);
            }
            Err(err) => {
                if let Error::NotFound(name, span) = err {
                    if let Some(dropped_name) =
                        self.dropped_names.iter().find(|n| n.name.as_ref() == name)
                    {
                        self.errors.push(Error::NotAvailable(
                            name,
                            format!(
                                "{}.{}",
                                dropped_name
                                    .namespace
                                    .iter()
                                    .map(std::string::ToString::to_string)
                                    .collect::<Vec<_>>()
                                    .join("."),
                                dropped_name.name
                            ),
                            span,
                        ));
                    } else {
                        self.errors.push(Error::NotFound(name, span));
                    }
                } else {
                    self.errors.push(err);
                }
            }
        }
    }

    /// # Arguments
    ///
    /// * `pat` - The pattern to bind.
    /// * `valid_at` - The offset at which the name becomes defined. This is used to determine
    ///   whether a name is in scope at a given offset.
    ///   e.g. For a local variable, this would be immediately after the declaration statement.
    ///   For input parameters to a callable, this would be the start of the body block.
    fn bind_pat(&mut self, pat: &ast::Pat, valid_at: u32) {
        let mut bindings = FxHashSet::default();
        self.bind_pat_recursive(pat, valid_at, &mut bindings);
    }

    fn bind_pat_recursive(
        &mut self,
        pat: &ast::Pat,
        valid_at: u32,
        bindings: &mut FxHashSet<Rc<str>>,
    ) {
        match &*pat.kind {
            ast::PatKind::Bind(name, _) => {
                if !bindings.insert(Rc::clone(&name.name)) {
                    self.errors
                        .push(Error::DuplicateBinding(name.name.to_string(), name.span));
                }
                self.names.insert(name.id, Res::Local(name.id));
                self.current_scope_mut()
                    .vars
                    .insert(Rc::clone(&name.name), (valid_at, name.id));
            }
            ast::PatKind::Discard(_) | ast::PatKind::Elided | ast::PatKind::Err => {}
            ast::PatKind::Paren(pat) => self.bind_pat_recursive(pat, valid_at, bindings),
            ast::PatKind::Tuple(pats) => pats
                .iter()
                .for_each(|p| self.bind_pat_recursive(p, valid_at, bindings)),
        }
    }

    fn bind_open(&mut self, name: &Idents, alias: &Option<Box<Ident>>) {
        let Some(id) = self.globals.find_namespace(name.str_iter()) else {
            self.errors.push(Error::NotFound(
                name.iter()
                    .map(|x| x.name.to_string())
                    .collect::<Vec<_>>()
                    .join("."),
                name.span(),
            ));
            return;
        };
        let alias = alias
            .as_ref()
            .map_or(name.into(), |a| vec![Rc::clone(&a.name)]);
        if self
            .globals
            .namespaces
            .get_namespace_id(name.str_iter())
            .is_some()
        {
            self.current_scope_mut()
                .opens
                .entry(alias)
                .or_default()
                .push(Open {
                    namespace: id,
                    span: name.span(),
                });
        } else {
            self.errors
                .push(Error::NotFound(name.to_string(), name.span()));
        }
    }

    pub(super) fn bind_local_item(&mut self, assigner: &mut Assigner, item: &ast::Item) {
        match &*item.kind {
            ast::ItemKind::Open(name, alias) => self.bind_open(name, alias),
            ast::ItemKind::Callable(decl) => {
                let id = intrapackage(assigner.next_item());
                self.names.insert(
                    decl.name.id,
                    Res::Item(
                        id,
                        ItemStatus::from_attrs(&ast_attrs_as_hir_attrs(&item.attrs)),
                    ),
                );
                self.current_scope_mut()
                    .terms
                    .insert(Rc::clone(&decl.name.name), id);
            }
            ast::ItemKind::Ty(name, _) => {
                let id = intrapackage(assigner.next_item());
                self.names.insert(
                    name.id,
                    Res::Item(
                        id,
                        ItemStatus::from_attrs(&ast_attrs_as_hir_attrs(&item.attrs)),
                    ),
                );
                let scope = self.current_scope_mut();
                scope.tys.insert(Rc::clone(&name.name), id);
                scope.terms.insert(Rc::clone(&name.name), id);
            }
            ast::ItemKind::Err => {}
        }
    }

    fn bind_type_parameters(&mut self, decl: &CallableDecl) {
        decl.generics.iter().enumerate().for_each(|(ix, ident)| {
            self.current_scope_mut()
                .ty_vars
                .insert(Rc::clone(&ident.name), ix.into());
            self.names.insert(ident.id, Res::Param(ix.into()));
        });
    }

    fn push_scope(&mut self, span: Span, kind: ScopeKind) {
        let scope_id = self.locals.push_scope(Scope::new(kind, span));
        self.curr_scope_chain.push(scope_id);
    }

    fn pop_scope(&mut self) {
        self.curr_scope_chain
            .pop()
            .expect("pushed scope should be the last element on the stack");
    }

    /// Returns the innermost scope in the current scope chain.
    fn current_scope_mut(&mut self) -> &mut Scope {
        let scope_id = *self
            .curr_scope_chain
            .last()
            .expect("there should be at least one scope at location");

        self.locals.get_scope_mut(scope_id)
    }

    pub(crate) fn namespaces(&self) -> &NamespaceTreeRoot {
        &self.globals.namespaces
    }
}

/// Constructed from a [Resolver] and an [Assigner], this structure implements `Visitor`
/// on the package AST where it resolves identifiers, and assigns IDs to them.
pub(super) struct With<'a> {
    resolver: &'a mut Resolver,
    assigner: &'a mut Assigner,
}

impl With<'_> {
    /// Given a function, apply that function to `Self` within a scope. In other words, this
    /// function automatically pushes a scope before `f` and pops it after.
    fn with_scope(&mut self, span: Span, kind: ScopeKind, f: impl FnOnce(&mut Self)) {
        self.resolver.push_scope(span, kind);
        f(self);
        self.resolver.pop_scope();
    }

    /// Apply `f` to self while a pattern's constituent identifiers are in scope. Removes those
    /// identifiers from the scope after `f`.
    fn with_pat(&mut self, span: Span, kind: ScopeKind, pat: &ast::Pat, f: impl FnOnce(&mut Self)) {
        self.with_scope(span, kind, |visitor| {
            // The bindings are valid from the beginning of the scope
            visitor.resolver.bind_pat(pat, span.lo);
            f(visitor);
        });
    }

    fn with_spec_pat(
        &mut self,
        span: Span,
        kind: ScopeKind,
        pat: &ast::Pat,
        f: impl FnOnce(&mut Self),
    ) {
        let mut bindings = self
            .resolver
            .curr_params
            .as_ref()
            .map_or_else(FxHashSet::default, std::clone::Clone::clone);
        self.with_scope(span, kind, |visitor| {
            visitor
                .resolver
                .bind_pat_recursive(pat, span.lo, &mut bindings);
            f(visitor);
        });
    }
}

impl AstVisitor<'_> for With<'_> {
    fn visit_namespace(&mut self, namespace: &ast::Namespace) {
        let ns = self
            .resolver
            .globals
            .find_namespace(namespace.name.str_iter())
            .expect("namespace should exist by this point");

        let kind = ScopeKind::Namespace(ns);
        self.with_scope(namespace.span, kind, |visitor| {
            // the below line ensures that this namespace opens itself, in case
            // we are re-opening a namespace. This is important, as without this,
            // a re-opened namespace would only have knowledge of its scopes.
            visitor.resolver.bind_open(&namespace.name, &None);
            for item in &*namespace.items {
                if let ast::ItemKind::Open(name, alias) = &*item.kind {
                    visitor.resolver.bind_open(name, alias);
                }
            }
            ast_visit::walk_namespace(visitor, namespace);
        });
    }

    fn visit_attr(&mut self, attr: &ast::Attr) {
        // The Config attribute arguments do not go through name resolution.
        if hir::Attr::from_str(attr.name.name.as_ref()) != Ok(hir::Attr::Config) {
            walk_attr(self, attr);
        }
    }

    fn visit_callable_decl(&mut self, decl: &CallableDecl) {
        fn collect_param_names(pat: &ast::Pat, names: &mut FxHashSet<Rc<str>>) {
            match &*pat.kind {
                ast::PatKind::Bind(name, _) => {
                    names.insert(Rc::clone(&name.name));
                }
                ast::PatKind::Discard(_) | ast::PatKind::Elided | ast::PatKind::Err => {}
                ast::PatKind::Paren(pat) => collect_param_names(pat, names),
                ast::PatKind::Tuple(pats) => {
                    pats.iter().for_each(|p| collect_param_names(p, names));
                }
            }
        }
        let mut param_names = FxHashSet::default();
        collect_param_names(&decl.input, &mut param_names);
        let prev_param_names = self.resolver.curr_params.replace(param_names);
        self.with_scope(decl.span, ScopeKind::Callable, |visitor| {
            visitor.resolver.bind_type_parameters(decl);
            // The parameter bindings are valid after the end of the input pattern.
            // (More accurately, in the callable body, but we don't have a start offset for that).
            visitor.resolver.bind_pat(&decl.input, decl.input.span.hi);
            ast_visit::walk_callable_decl(visitor, decl);
        });
        self.resolver.curr_params = prev_param_names;
    }

    fn visit_spec_decl(&mut self, decl: &ast::SpecDecl) {
        if let SpecBody::Impl(input, block) = &decl.body {
            self.with_spec_pat(block.span, ScopeKind::Block, input, |visitor| {
                visitor.visit_block(block);
            });
        } else {
            ast_visit::walk_spec_decl(self, decl);
        }
    }

    fn visit_ty(&mut self, ty: &ast::Ty) {
        match &*ty.kind {
            ast::TyKind::Path(path) => {
                self.resolver.resolve_path(NameKind::Ty, path);
            }
            ast::TyKind::Param(ident) => {
                self.resolver.resolve_ident(NameKind::Ty, ident);
            }
            _ => ast_visit::walk_ty(self, ty),
        }
    }

    fn visit_block(&mut self, block: &ast::Block) {
        self.with_scope(block.span, ScopeKind::Block, |visitor| {
            for stmt in &*block.stmts {
                if let ast::StmtKind::Item(item) = &*stmt.kind {
                    visitor.resolver.bind_local_item(visitor.assigner, item);
                }
            }

            ast_visit::walk_block(visitor, block);
        });
    }

    fn visit_stmt(&mut self, stmt: &ast::Stmt) {
        match &*stmt.kind {
            ast::StmtKind::Item(item) => self.visit_item(item),
            ast::StmtKind::Local(_, pat, _) => {
                ast_visit::walk_stmt(self, stmt);
                // The binding is valid after end of the statement.
                self.resolver.bind_pat(pat, stmt.span.hi);
            }
            ast::StmtKind::Qubit(_, pat, init, block) => {
                ast_visit::walk_qubit_init(self, init);
                if let Some(block) = block {
                    self.with_pat(block.span, ScopeKind::Block, pat, |visitor| {
                        visitor.visit_block(block);
                    });
                } else {
                    // The binding is valid after end of the statement.
                    self.resolver.bind_pat(pat, stmt.span.hi);
                }
            }
            ast::StmtKind::Empty
            | ast::StmtKind::Expr(_)
            | ast::StmtKind::Semi(_)
            | ast::StmtKind::Err => {
                ast_visit::walk_stmt(self, stmt);
            }
        }
    }

    fn visit_expr(&mut self, expr: &ast::Expr) {
        match &*expr.kind {
            ast::ExprKind::For(pat, iter, block) => {
                self.visit_expr(iter);
                self.with_pat(block.span, ScopeKind::Block, pat, |visitor| {
                    visitor.visit_block(block);
                });
            }
            ast::ExprKind::Lambda(_, input, output) => {
                self.with_pat(output.span, ScopeKind::Block, input, |visitor| {
                    visitor.visit_expr(output);
                });
            }
            ast::ExprKind::Path(path) => self.resolver.resolve_path(NameKind::Term, path),
            ast::ExprKind::TernOp(ast::TernOp::Update, container, index, replace)
            | ast::ExprKind::AssignUpdate(container, index, replace) => {
                self.visit_expr(container);
                if !is_field_update(
                    &self.resolver.globals,
                    self.resolver
                        .locals
                        .get_scopes(&self.resolver.curr_scope_chain),
                    index,
                ) {
                    self.visit_expr(index);
                }
                self.visit_expr(replace);
            }
            _ => ast_visit::walk_expr(self, expr),
        }
    }
}

pub(super) struct GlobalTable {
    names: Names,
    scope: GlobalScope,
}

impl GlobalTable {
    pub(super) fn new() -> Self {
        let builtins: [(Rc<str>, Res); 10] = [
            ("BigInt".into(), Res::PrimTy(Prim::BigInt)),
            ("Bool".into(), Res::PrimTy(Prim::Bool)),
            ("Double".into(), Res::PrimTy(Prim::Double)),
            ("Int".into(), Res::PrimTy(Prim::Int)),
            ("Pauli".into(), Res::PrimTy(Prim::Pauli)),
            ("Qubit".into(), Res::PrimTy(Prim::Qubit)),
            ("Range".into(), Res::PrimTy(Prim::Range)),
            ("Result".into(), Res::PrimTy(Prim::Result)),
            ("String".into(), Res::PrimTy(Prim::String)),
            ("Unit".into(), Res::UnitTy),
        ];
        let mut core: FxHashMap<Rc<str>, Res> = FxHashMap::default();
        for (name, res) in builtins {
            core.insert(name, res);
        }

        let mut scope = GlobalScope::default();
        let ns = scope.insert_or_find_namespace(vec![
            Rc::from("Microsoft"),
            Rc::from("Quantum"),
            Rc::from("Core"),
        ]);

        let mut tys = FxHashMap::default();
        tys.insert(ns, core);

        Self {
            names: IndexMap::new(),
            scope: GlobalScope {
                tys,
                terms: FxHashMap::default(),
                namespaces: NamespaceTreeRoot::default(),
                intrinsics: FxHashSet::default(),
            },
        }
    }

    pub(super) fn add_local_package(
        &mut self,
        assigner: &mut Assigner,
        package: &ast::Package,
    ) -> Vec<Error> {
        let mut errors = Vec::new();
        for node in &*package.nodes {
            match node {
                TopLevelNode::Namespace(namespace) => {
                    bind_global_items(
                        &mut self.names,
                        &mut self.scope,
                        namespace,
                        assigner,
                        &mut errors,
                    );
                }
                TopLevelNode::Stmt(_) => {
                    unimplemented!("did not expect top-level statements in the ast")
                }
            }
        }
        errors
    }

    pub(super) fn add_external_package(&mut self, id: PackageId, package: &hir::Package) {
        for global in global::iter_package(Some(id), package).filter(|global| {
            global.visibility == hir::Visibility::Public
                || matches!(&global.kind, global::Kind::Term(t) if t.intrinsic)
        }) {
            let namespace = self
                .scope
                .insert_or_find_namespace(global.namespace.clone());

            match (global.kind, global.visibility) {
                (global::Kind::Ty(ty), hir::Visibility::Public) => {
                    self.scope
                        .tys
                        .entry(namespace)
                        .or_default()
                        .insert(global.name, Res::Item(ty.id, global.status));
                }
                (global::Kind::Term(term), visibility) => {
                    if visibility == hir::Visibility::Public {
                        self.scope
                            .terms
                            .entry(namespace)
                            .or_default()
                            .insert(global.name.clone(), Res::Item(term.id, global.status));
                    }
                    if term.intrinsic {
                        self.scope.intrinsics.insert(global.name);
                    }
                }
                (global::Kind::Namespace, hir::Visibility::Public) => {
                    self.scope.insert_or_find_namespace(global.namespace);
                }
                (_, hir::Visibility::Internal) => {}
            }
        }
    }
}

/// Given some namespace `namespace`, add all the globals declared within it to the global scope.
fn bind_global_items(
    names: &mut IndexMap<NodeId, Res>,
    scope: &mut GlobalScope,
    namespace: &ast::Namespace,
    assigner: &mut Assigner,
    errors: &mut Vec<Error>,
) {
    names.insert(
        namespace.id,
        Res::Item(intrapackage(assigner.next_item()), ItemStatus::Available),
    );

    let namespace_id = scope.insert_or_find_namespace(&namespace.name);

    for item in &*namespace.items {
        match bind_global_item(
            names,
            scope,
            namespace_id,
            || intrapackage(assigner.next_item()),
            item,
        ) {
            Ok(()) => {}
            Err(mut e) => errors.append(&mut e),
        }
    }
}

/// Tries to extract a field name from an expression in cases where it is syntactically ambiguous
/// whether the expression is a field name or a variable name. This applies to the index operand in
/// a ternary update operator.
pub(super) fn extract_field_name<'a>(names: &Names, expr: &'a ast::Expr) -> Option<&'a Rc<str>> {
    // Follow the same reasoning as `is_field_update`.
    match &*expr.kind {
        ast::ExprKind::Path(path)
            if path.namespace.is_none() && !matches!(names.get(path.id), Some(Res::Local(_))) =>
        {
            Some(&path.name.name)
        }
        _ => None,
    }
}

fn is_field_update<'a>(
    globals: &GlobalScope,
    scopes: impl Iterator<Item = &'a Scope>,
    index: &ast::Expr,
) -> bool {
    // Disambiguate the update operator by looking at the index expression. If it's an
    // unqualified path that doesn't resolve to a local, assume that it's meant to be a field name.
    match &*index.kind {
        ast::ExprKind::Path(path) if path.namespace.is_none() => !matches!(
            {
                let name = &path.name;
                let namespace = &path.namespace;
                resolve(NameKind::Term, globals, scopes, name, namespace)
            },
            Ok(Res::Local(_))
        ),
        _ => false,
    }
}

fn ast_attrs_as_hir_attrs(attrs: &[Box<ast::Attr>]) -> Vec<hir::Attr> {
    attrs
        .iter()
        .filter_map(|attr| hir::Attr::from_str(attr.name.name.as_ref()).ok())
        .collect()
}

fn bind_global_item(
    names: &mut Names,
    scope: &mut GlobalScope,
    namespace: NamespaceId,
    next_id: impl FnOnce() -> ItemId,
    item: &ast::Item,
) -> Result<(), Vec<Error>> {
    match &*item.kind {
        ast::ItemKind::Callable(decl) => {
            let item_id = next_id();
            let status = ItemStatus::from_attrs(&ast_attrs_as_hir_attrs(item.attrs.as_ref()));
            let res = Res::Item(item_id, status);
            names.insert(decl.name.id, res);
            let mut errors = Vec::new();
            match scope
                .terms
                .entry(namespace)
                .or_default()
                .entry(Rc::clone(&decl.name.name))
            {
                Entry::Occupied(_) => {
                    let namespace_name = scope
                        .namespaces
                        .find_namespace_by_id(&namespace)
                        .0
                        .join(".");
                    errors.push(Error::Duplicate(
                        decl.name.name.to_string(),
                        namespace_name.to_string(),
                        decl.name.span,
                    ));
                }
                Entry::Vacant(entry) => {
                    entry.insert(res);
                }
            }

            if decl_is_intrinsic(decl) && !scope.intrinsics.insert(Rc::clone(&decl.name.name)) {
                errors.push(Error::DuplicateIntrinsic(
                    decl.name.name.to_string(),
                    decl.name.span,
                ));
            }

            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors)
            }
        }
        ast::ItemKind::Ty(name, _) => {
            let item_id = next_id();
            let status = ItemStatus::from_attrs(&ast_attrs_as_hir_attrs(item.attrs.as_ref()));
            let res = Res::Item(item_id, status);
            names.insert(name.id, res);
            match (
                scope
                    .terms
                    .entry(namespace)
                    .or_default()
                    .entry(Rc::clone(&name.name)),
                scope
                    .tys
                    .entry(namespace)
                    .or_default()
                    .entry(Rc::clone(&name.name)),
            ) {
                (Entry::Occupied(_), _) | (_, Entry::Occupied(_)) => {
                    let namespace_name = scope
                        .namespaces
                        .find_namespace_by_id(&namespace)
                        .0
                        .join(".");
                    Err(vec![Error::Duplicate(
                        name.name.to_string(),
                        namespace_name,
                        name.span,
                    )])
                }
                (Entry::Vacant(term_entry), Entry::Vacant(ty_entry)) => {
                    term_entry.insert(res);
                    ty_entry.insert(res);
                    Ok(())
                }
            }
        }
        ast::ItemKind::Err | ast::ItemKind::Open(..) => Ok(()),
    }
}

fn decl_is_intrinsic(decl: &CallableDecl) -> bool {
    if let CallableBody::Specs(specs) = decl.body.as_ref() {
        specs
            .iter()
            .any(|spec| matches!(spec.body, SpecBody::Gen(SpecGen::Intrinsic)))
    } else {
        false
    }
}

/// Resolves a given symbol and namespace name, according to the Q# shadowing rules.
/// Shadowing rules are as follows:
/// - Local variables shadow everything. They are the first priority.
/// - Next, we check open statements for an explicit open.
/// - Then, we check the prelude.
/// - Lastly, we check the global namespace.
/// In the example `Foo.Bar.Baz()` -- the `provided_namespace_name` would be
///`Foo.Bar` and the `provided_symbol_name` would be `Baz`.
///
/// In the example `Foo()` -- the `provided_namespace_name` would be `None` and the
/// `provided_symbol_name` would be `Foo`.
/// returns the resolution if successful, or an error if not.
fn resolve<'a>(
    kind: NameKind,
    globals: &GlobalScope,
    scopes: impl Iterator<Item = &'a Scope>,
    provided_symbol_name: &Ident,
    provided_namespace_name: &Option<Idents>,
) -> Result<Res, Error> {
    // let scopes = scopes.collect::<Vec<_>>();

    if let Some(value) = check_all_scopes(
        kind,
        globals,
        provided_symbol_name,
        provided_namespace_name,
        scopes,
    ) {
        return value;
    }

    // check the prelude
    if provided_namespace_name.is_none() {
        let prelude_candidates = find_symbol_in_namespaces(
            kind,
            globals,
            provided_namespace_name,
            provided_symbol_name,
            prelude_namespaces(globals).into_iter().map(|(a, b)| (b, a)),
            // there are no aliases in the prelude
            &FxHashMap::default(),
        );

        if prelude_candidates.len() > 1 {
            // If there are multiple candidates, sort them by namespace and return an error.
            let candidates: Vec<_> = prelude_candidates.into_iter().collect();
            let mut candidates = candidates
                .into_iter()
                .map(|(_candidate, ns_name)| ns_name)
                .collect::<Vec<_>>();
            candidates.sort();

            let mut candidates = candidates.into_iter();
            let candidate_a = candidates
                .next()
                .expect("infallible as per length check above");
            let candidate_b = candidates
                .next()
                .expect("infallible as per length check above");
            return Err(Error::AmbiguousPrelude {
                span: provided_symbol_name.span,
                name: provided_symbol_name.name.to_string(),
                candidate_a,
                candidate_b,
            });
        }
        // if there is a candidate, return it
        if let Some((res, _)) = single(prelude_candidates) {
            return Ok(res);
        }
    }

    // lastly, check unopened globals
    let global_candidates = find_symbol_in_namespaces(
        kind,
        globals,
        provided_namespace_name,
        provided_symbol_name,
        std::iter::once((globals.namespaces.root_id(), ())),
        // there are no aliases in globals
        &FxHashMap::default(),
    );

    // we don't have to throw an error if there are extra candidates here, as we are only looking at the root,
    // and that's only one namespace. individual namespaces cannot have duplicate declarations.
    if let Some(res) = single(global_candidates.into_keys()) {
        return Ok(res);
    }

    Err(Error::NotFound(
        provided_symbol_name.name.to_string(),
        provided_symbol_name.span,
    ))
}
/// Checks all given scopes, in the correct order, for a resolution.
/// Calls `check_scoped_resolutions` on each scope, and tracks if we should allow local variables in closures in parent scopes
/// using the `vars` parameter.
fn check_all_scopes<'a>(
    kind: NameKind,
    globals: &GlobalScope,
    provided_symbol_name: &Ident,
    provided_namespace_name: &Option<Idents>,
    scopes: impl Iterator<Item = &'a Scope>,
) -> Option<Result<Res, Error>> {
    let mut vars = true;

    for scope in scopes {
        if let Some(value) = check_scoped_resolutions(
            kind,
            globals,
            provided_symbol_name,
            provided_namespace_name,
            &mut vars,
            scope,
        ) {
            return Some(value);
        }
    }
    None
}

/// This function checks scopes for a given symbol and namespace name.
/// In a given [Scope], check:
/// 1. if any locally declared symbols match `provided_symbol_name`
/// 2. if any aliases in this scope match the provided namespace, and if they contain `provided_symbol_name`
/// 3. if any opens in this scope contain the `provided_symbol_name`
/// It follows the Q# shadowing rules:
/// - Local variables shadow everything. They are the first priority.
/// - Next, we check open statements for an explicit open.
/// - Then, we check the prelude.
/// - Lastly, we check the global namespace.
///
/// # Parameters
///
/// * `kind` - The [`NameKind`] of the name
/// * `globals` - The global scope to resolve the name against.
/// * `provided_symbol_name` - The symbol name to resolve.
/// * `provided_namespace_name` - The namespace of the symbol, if any.
/// * `vars` - A mutable reference to a boolean indicating whether to allow local variables in closures in parent scopes.
/// * `scope` - The scope to check for resolutions.
fn check_scoped_resolutions(
    kind: NameKind,
    globals: &GlobalScope,
    provided_symbol_name: &Ident,
    provided_namespace_name: &Option<Idents>,
    vars: &mut bool,
    scope: &Scope,
) -> Option<Result<Res, Error>> {
    if provided_namespace_name.is_none() {
        if let Some(res) =
            resolve_scope_locals(kind, globals, scope, *vars, &provided_symbol_name.name)
        {
            // Local declarations shadow everything.
            return Some(Ok(res));
        }
    }
    let aliases = scope
        .opens
        .iter()
        .map(|(alias, opens)| {
            (
                alias.clone(),
                opens.iter().map(|x| (x.namespace, x)).collect(),
            )
        })
        .collect::<FxHashMap<_, _>>();

    let explicit_open_candidates = find_symbol_in_namespaces(
        kind,
        globals,
        provided_namespace_name,
        provided_symbol_name,
        scope
            .opens
            .iter()
            .flat_map(|(_, open)| open)
            .map(|open @ Open { namespace, .. }| (*namespace, open)),
        &aliases,
    );
    match explicit_open_candidates.len() {
        1 => {
            return Some(Ok(single(explicit_open_candidates.into_keys())
                .expect("we asserted on the length, so this is infallible")))
        }
        len if len > 1 => {
            return Some(ambiguous_symbol_error(
                globals,
                provided_symbol_name,
                explicit_open_candidates
                    .into_iter()
                    .map(|(a, b)| (a, b.clone()))
                    .collect(),
            ))
        }
        _ => (),
    }
    if scope.kind == ScopeKind::Callable {
        // Since local callables are not closures, hide local variables in parent scopes.
        *vars = false;
    }
    None
}

/// This function always returns an `Err` variant of `Result`. The error is of type `Error::Ambiguous` and contains
/// the name of the ambiguous symbol and the namespaces that contain the conflicting entities.
/// # Arguments
///
/// * `globals` - The global scope to resolve the name against.
/// * `provided_symbol_name` - The symbol name that is ambiguous.
/// * `candidates` - A map of possible resolutions for the symbol, each associated with the `Open`
/// statement that brought it into scope. Note that only the first two opens in
/// the candidates are actually used in the error message.
fn ambiguous_symbol_error(
    globals: &GlobalScope,
    provided_symbol_name: &Ident,
    candidates: FxHashMap<Res, Open>,
) -> Result<Res, Error> {
    let mut opens: Vec<_> = candidates.into_values().collect();
    opens.sort_unstable_by_key(|open| open.span);
    let (first_open_ns, _) = globals.namespaces.find_namespace_by_id(&opens[0].namespace);
    let (second_open_ns, _) = globals.namespaces.find_namespace_by_id(&opens[1].namespace);
    Err(Error::Ambiguous {
        name: provided_symbol_name.name.to_string(),
        first_open: first_open_ns.join("."),
        second_open: second_open_ns.join("."),
        name_span: provided_symbol_name.span,
        first_open_span: opens[0].span,
        second_open_span: opens[1].span,
    })
}

fn find_symbol_in_namespaces<T, O>(
    kind: NameKind,
    globals: &GlobalScope,
    provided_namespace_name: &Option<Idents>,
    provided_symbol_name: &Ident,
    namespaces_to_search: T,
    aliases: &FxHashMap<Vec<Rc<str>>, Vec<(NamespaceId, O)>>,
) -> FxHashMap<Res, O>
where
    T: Iterator<Item = (NamespaceId, O)>,
    O: Clone + std::fmt::Debug,
{
    // check aliases to see if the provided namespace is actually an alias
    if let Some(provided_namespace_name) = provided_namespace_name {
        if let Some(opens) = aliases.get(&(Into::<Vec<Rc<_>>>::into(provided_namespace_name))) {
            let candidates: FxHashMap<_, _> = opens
                .iter()
                .filter_map(|(ns_id, open)| {
                    globals
                        .get(kind, *ns_id, &provided_symbol_name.name)
                        .map(|res| (*res, open.clone()))
                })
                .collect();
            if !candidates.is_empty() {
                return candidates;
            }
        }
    }

    let mut candidates = FxHashMap::default();
    for (candidate_namespace_id, open) in namespaces_to_search {
        // Retrieve the namespace associated with the candidate_namespace_id from the global namespaces
        let candidate_namespace = globals
            .namespaces
            .find_namespace_by_id(&candidate_namespace_id)
            .1;

        // Attempt to find a namespace within the candidate_namespace that matches the provided_namespace_name
        let namespace = provided_namespace_name.as_ref().and_then(|name| {
            candidate_namespace
                .borrow()
                .get_namespace_id(name.str_iter())
        });

        // if a namespace was provided, but not found, then this is not the correct namespace.
        if provided_namespace_name.is_some() && namespace.is_none() {
            continue;
        }

        // Attempt to get the symbol from the global scope. If the namespace is None, use the candidate_namespace_id as a fallback
        let res = namespace
            .or(if namespace.is_none() {
                Some(candidate_namespace_id)
            } else {
                None
            })
            .and_then(|ns_id| globals.get(kind, ns_id, &provided_symbol_name.name));

        // If a symbol was found, insert it into the candidates map
        if let Some(res) = res {
            candidates.insert(*res, open);
        }
    }

    if candidates.len() > 1 {
        // If there are multiple candidates, remove unimplemented items. This allows resolution to
        // succeed in cases where both an older, unimplemented API and newer, implemented API with the
        // same name are both in scope without forcing the user to fully qualify the name.
        candidates.retain(|&res, _| !matches!(res, Res::Item(_, ItemStatus::Unimplemented)));
    }
    candidates
}

/// Fetch the name and namespace ID of all prelude namespaces.
fn prelude_namespaces(globals: &GlobalScope) -> Vec<(String, NamespaceId)> {
    let mut prelude = Vec::with_capacity(PRELUDE.len());

    // add prelude to the list of candidate namespaces last, as they are the final fallback for a symbol
    for prelude_namespace in PRELUDE {
        prelude.push((
            prelude_namespace.join("."),
            globals
                .namespaces
                .get_namespace_id(prelude_namespace)
                .expect("prelude should always exist in the namespace map"),
        ));
    }
    prelude
}
/// Implements shadowing rules within a single scope.
/// A local variable always wins out against an item with the same name, even if they're declared in
/// the same scope. It is implemented in a way that resembles Rust:
/// ```rust
/// let foo = || 1;
/// fn foo() -> i32 { 2 }
/// dbg!(foo()); // 1, not 2
/// ```
fn resolve_scope_locals(
    kind: NameKind,
    globals: &GlobalScope,
    scope: &Scope,
    vars: bool,
    name: &str,
) -> Option<Res> {
    if vars {
        match kind {
            NameKind::Term => {
                if let Some(&(_, id)) = scope.vars.get(name) {
                    return Some(Res::Local(id));
                }
            }
            NameKind::Ty => {
                if let Some(&id) = scope.ty_vars.get(name) {
                    return Some(Res::Param(id));
                }
            }
        }
    }

    if let Some(&id) = scope.item(kind, name) {
        return Some(Res::Item(id, ItemStatus::Available));
    }

    if let ScopeKind::Namespace(namespace) = &scope.kind {
        if let Some(&res) = globals.get(kind, *namespace, name) {
            return Some(res);
        }
    }

    None
}

fn get_scope_locals(scope: &Scope, offset: u32, vars: bool) -> Vec<Local> {
    let mut names = Vec::new();

    // variables
    if vars {
        names.extend(scope.vars.iter().filter_map(|(name, (valid_at, id))| {
            // Bug: Because we keep track of only one `valid_at` offset per name,
            // when a variable is later shadowed in the same scope,
            // it is missed in the list. https://github.com/microsoft/qsharp/issues/897
            if offset >= *valid_at {
                Some(Local {
                    name: name.clone(),
                    kind: LocalKind::Var(*id),
                })
            } else {
                None
            }
        }));

        names.extend(scope.ty_vars.iter().map(|id| Local {
            name: id.0.clone(),
            kind: LocalKind::TyParam(*id.1),
        }));
    }

    // items
    // skip adding newtypes since they're already in the terms map
    names.extend(scope.terms.iter().map(|term| Local {
        name: term.0.clone(),
        kind: LocalKind::Item(*term.1),
    }));

    names
}

/// Creates an [`ItemId`] for an item that is local to this package (internal to it).
fn intrapackage(item: LocalItemId) -> ItemId {
    ItemId {
        package: None,
        item,
    }
}

fn single<T>(xs: impl IntoIterator<Item = T>) -> Option<T> {
    let mut xs = xs.into_iter();
    let x = xs.next();
    match xs.next() {
        None => x,
        Some(_) => None,
    }
}
