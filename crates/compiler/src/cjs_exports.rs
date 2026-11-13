//! Extract named exports from `CommonJS` module source.
//!
//! Rolldown's built-in CJS-to-ESM interop extracts named exports only for
//! top-level `exports.X = ...` statements. React 19.3.1 wraps its 46
//! `exports.X = ...` assignments in a `"production" !== process.env.NODE_ENV
//! && (function(){...})();` IIFE, so the built-in analysis sees nothing —
//! the bundle ends with `export default require_react_development();` and no
//! named exports.
//!
//! This module replicates `cjs-module-lexer`'s walk: parse the source, walk
//! every nested node, and collect the export names wherever they appear.
//! Handles the three shapes that cover the React ecosystem:
//!
//! 1. `exports.NAME = ...` (React, React-DOM, JSX runtime)
//! 2. `module.exports.NAME = ...` (`CommonJS` alternatives)
//! 3. `module.exports = { NAME: ..., OTHER: ... }` (object-literal entry)
//!
//! The caller (see `prebundle.rs::bundle_one`) uses this to append missing
//! `export const NAME = __oxide_cjs_default.NAME;` lines to Rolldown's
//! output — the standard "post-process a bundle with the CJS analysis it
//! should have done" pattern that Vite uses for `depOptimizer`.

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionBody, ArrowFunctionExpression, AssignmentExpression, AssignmentTarget,
    BlockStatement, CallExpression, Expression, Function, FunctionBody, ObjectExpression,
    ObjectPropertyKind, Program, PropertyKey, Statement, TryStatement,
};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::collections::BTreeSet;

const MAX_DEPTH: u32 = 64;

/// Extract every named export from a `CommonJS` module's source. Returns a
/// sorted, deduplicated list of identifier-safe names. `default` is
/// excluded (the bundle's own `export default X;` covers it).
///
/// Returns an empty `Vec` on parse failure — the caller leaves the bundle
/// untouched in that case rather than inventing exports from an unclear
/// source.
#[must_use]
pub fn extract_cjs_export_names(source: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();
    if !parsed.diagnostics.is_empty() {
        return Vec::new();
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    walk_program(&parsed.program, &mut names);
    names
        .into_iter()
        .filter(|n| n != "default" && is_valid_js_ident(n))
        .collect()
}

fn is_valid_js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

// --- statement walk ------------------------------------------------------

fn walk_program(program: &Program<'_>, out: &mut BTreeSet<String>) {
    for stmt in &program.body {
        walk_statement(stmt, out, 0);
    }
}

fn walk_statement(stmt: &Statement<'_>, out: &mut BTreeSet<String>, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    match stmt {
        Statement::ExpressionStatement(s) => walk_expression(&s.expression, out, depth + 1),
        Statement::BlockStatement(s) => walk_block(s, out, depth + 1),
        Statement::IfStatement(s) => {
            walk_expression(&s.test, out, depth + 1);
            walk_statement(&s.consequent, out, depth + 1);
            if let Some(alt) = &s.alternate {
                walk_statement(alt, out, depth + 1);
            }
        }
        Statement::TryStatement(s) => walk_try(s, out, depth + 1),
        Statement::ReturnStatement(s) => {
            if let Some(arg) = &s.argument {
                walk_expression(arg, out, depth + 1);
            }
        }
        Statement::WhileStatement(s) => {
            walk_expression(&s.test, out, depth + 1);
            walk_statement(&s.body, out, depth + 1);
        }
        Statement::DoWhileStatement(s) => {
            walk_statement(&s.body, out, depth + 1);
            walk_expression(&s.test, out, depth + 1);
        }
        Statement::ForStatement(s) => walk_statement(&s.body, out, depth + 1),
        Statement::ForInStatement(s) => walk_statement(&s.body, out, depth + 1),
        Statement::ForOfStatement(s) => walk_statement(&s.body, out, depth + 1),
        Statement::ThrowStatement(s) => walk_expression(&s.argument, out, depth + 1),
        Statement::SwitchStatement(s) => {
            for case in &s.cases {
                for stmt in &case.consequent {
                    walk_statement(stmt, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn walk_block(block: &BlockStatement<'_>, out: &mut BTreeSet<String>, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    for stmt in &block.body {
        walk_statement(stmt, out, depth + 1);
    }
}

fn walk_try(stmt: &TryStatement<'_>, out: &mut BTreeSet<String>, depth: u32) {
    walk_block(&stmt.block, out, depth + 1);
    if let Some(handler) = &stmt.handler {
        walk_block(&handler.body, out, depth + 1);
    }
    if let Some(finalizer) = &stmt.finalizer {
        walk_block(finalizer, out, depth + 1);
    }
}

// --- expression walk -----------------------------------------------------

fn walk_expression(expr: &Expression<'_>, out: &mut BTreeSet<String>, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    match expr {
        Expression::AssignmentExpression(a) => walk_assignment(a, out, depth + 1),
        Expression::LogicalExpression(l) => {
            walk_expression(&l.left, out, depth + 1);
            walk_expression(&l.right, out, depth + 1);
        }
        Expression::BinaryExpression(b) => {
            walk_expression(&b.left, out, depth + 1);
            walk_expression(&b.right, out, depth + 1);
        }
        Expression::ConditionalExpression(c) => {
            walk_expression(&c.test, out, depth + 1);
            walk_expression(&c.consequent, out, depth + 1);
            walk_expression(&c.alternate, out, depth + 1);
        }
        Expression::UnaryExpression(u) => walk_expression(&u.argument, out, depth + 1),
        Expression::AwaitExpression(a) => walk_expression(&a.argument, out, depth + 1),
        Expression::ParenthesizedExpression(p) => walk_expression(&p.expression, out, depth + 1),
        Expression::SequenceExpression(s) => {
            for e in &s.expressions {
                walk_expression(e, out, depth + 1);
            }
        }
        Expression::CallExpression(c) => walk_call(c, out, depth + 1),
        Expression::FunctionExpression(f) => walk_function(f, out, depth + 1),
        Expression::ArrowFunctionExpression(a) => walk_arrow(a, out, depth + 1),
        // An object literal at this position is a value, not an export
        // container. Its keys are only collected when it's the RHS of
        // `module.exports =` (handled in walk_assignment); every other
        // expression shape is equally a non-container.
        _ => {}
    }
}

fn walk_assignment(a: &AssignmentExpression<'_>, out: &mut BTreeSet<String>, depth: u32) {
    // Pattern 1 & 2: `exports.X = ...` / `module.exports.X = ...`
    if let Some(name) = assignment_target_name(&a.left) {
        out.insert(name);
    } else if is_module_exports(&a.left) {
        // Pattern 3: `module.exports = {...}` — collect all literal keys.
        if let Expression::ObjectExpression(o) = &a.right {
            collect_object_keys(o, out);
        }
    }
    // Always walk the RHS: `exports.foo = (function(){ ... exports.bar = ... })()`
    // is a real pattern (rare but valid).
    walk_expression(&a.right, out, depth + 1);
}

fn walk_call(c: &CallExpression<'_>, out: &mut BTreeSet<String>, depth: u32) {
    // Recurse into the callee (this is what descends into IIFEs:
    // `(function(){...})()` -> callee is FunctionExpression -> body).
    walk_expression(&c.callee, out, depth + 1);
    // Also recurse into arguments. `Argument` inherits from `Expression`
    // (flattened by `#[ast]`), so a real expression is reached via
    // `as_expression()`. A `SpreadElement` argument has no expression to
    // descend into.
    for arg in &c.arguments {
        if let Some(e) = arg.as_expression() {
            walk_expression(e, out, depth + 1);
        }
    }
}

fn walk_function(f: &Function<'_>, out: &mut BTreeSet<String>, depth: u32) {
    if let Some(body) = &f.body {
        walk_function_body(body, out, depth + 1);
    }
}

fn walk_function_body(body: &FunctionBody<'_>, out: &mut BTreeSet<String>, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    for stmt in &body.statements {
        walk_statement(stmt, out, depth + 1);
    }
}

fn walk_arrow(a: &ArrowFunctionExpression<'_>, out: &mut BTreeSet<String>, depth: u32) {
    // `ArrowFunctionBody` inherits from `Expression`; a function-body arrow
    // hits the `FunctionBody` variant, and an expression-body arrow
    // resolves through `as_expression()`.
    if let ArrowFunctionBody::FunctionBody(b) = &a.body {
        walk_function_body(b, out, depth + 1);
        return;
    }
    if let Some(e) = a.body.as_expression() {
        walk_expression(e, out, depth + 1);
    }
}

// --- pattern helpers -----------------------------------------------------

/// Returns `Some(NAME)` when `t` is `exports.NAME` or `module.exports.NAME`.
fn assignment_target_name(t: &AssignmentTarget<'_>) -> Option<String> {
    // `AssignmentTarget` flattens `SimpleAssignmentTarget` which flattens
    // `MemberExpression`. After the `#[ast]` macro runs, the variants are
    // direct: `AssignmentTarget::StaticMemberExpression(m)`, not nested
    // `INHERIT(...)` wrappers.
    let AssignmentTarget::StaticMemberExpression(m) = t else {
        return None;
    };
    let prop = m.property.name.as_str().to_string();
    // exports.NAME
    if let Expression::Identifier(id) = &m.object
        && id.name.as_str() == "exports"
    {
        return Some(prop);
    }
    // module.exports.NAME
    if let Expression::StaticMemberExpression(inner) = &m.object
        && inner.property.name.as_str() == "exports"
        && let Expression::Identifier(id) = &inner.object
        && id.name.as_str() == "module"
    {
        return Some(prop);
    }
    None
}

/// Returns `true` if `t` is exactly `module.exports` (no `.NAME` suffix).
fn is_module_exports(t: &AssignmentTarget<'_>) -> bool {
    let AssignmentTarget::StaticMemberExpression(m) = t else {
        return false;
    };
    m.property.name.as_str() == "exports"
        && matches!(&m.object, Expression::Identifier(id) if id.name.as_str() == "module")
}

fn collect_object_keys(o: &ObjectExpression<'_>, out: &mut BTreeSet<String>) {
    for prop in &o.properties {
        let ObjectPropertyKind::ObjectProperty(p) = prop else {
            continue;
        };
        if let Some(name) = property_key_name(&p.key) {
            out.insert(name);
        }
    }
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        // `IdentifierName.name` is `oxc_str::js_str::JSStr`, whose
        // `as_str()` is infallible.
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_string()),
        // `StringLiteral.value.as_str()` returns `Option<&str>`: a string
        // with escapes that oxc cannot decode inline yields `None`. A
        // computed key with escapes is not a valid identifier-safe export
        // name anyway — skip it (`None`) so the caller filters it out.
        PropertyKey::StringLiteral(s) => s.value.as_str().map(str::to_string),
        _ => None,
    }
}

// --- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_toplevel_exports_assignments() {
        let src = r#"
            "use strict";
            exports.useState = function() {};
            exports.useEffect = function() {};
            exports.version = "1.0.0";
        "#;
        let names = extract_cjs_export_names(src);
        assert_eq!(names, vec!["useEffect", "useState", "version"]);
    }

    #[test]
    fn extracts_exports_inside_iife() {
        // React 19.3.1 shape: `exports.X = Y` inside a logical-and-wrapped IIFE.
        let src = r#"
            "use strict";
            "production" !== process.env.NODE_ENV &&
              (function () {
                exports.Fragment = Symbol.for("react.fragment");
                exports.useState = function(i) { return [i, function() {}]; };
                exports.useEffect = function() {};
              })();
        "#;
        let names = extract_cjs_export_names(src);
        assert!(names.contains(&"Fragment".to_string()), "names = {names:?}");
        assert!(names.contains(&"useState".to_string()), "names = {names:?}");
        assert!(
            names.contains(&"useEffect".to_string()),
            "names = {names:?}"
        );
    }

    #[test]
    fn extracts_module_exports_dot_name() {
        let src = r"
            module.exports.jsx = function(t, p) {};
            module.exports.jsxs = function(t, p) {};
        ";
        let names = extract_cjs_export_names(src);
        assert_eq!(names, vec!["jsx", "jsxs"]);
    }

    #[test]
    fn extracts_module_exports_object_literal() {
        let src = r#"
            module.exports = {
              createRoot: function() {},
              hydrateRoot: function() {},
              "version": "1.0.0"
            };
        "#;
        let names = extract_cjs_export_names(src);
        assert_eq!(names, vec!["createRoot", "hydrateRoot", "version"]);
    }

    #[test]
    fn excludes_default_and_invalid_idents() {
        let src = r#"
            exports.default = 1;
            exports["invalid-name"] = 2;
            exports._valid = 3;
            exports.$also_valid = 4;
        "#;
        let names = extract_cjs_export_names(src);
        assert!(!names.contains(&"default".to_string()));
        assert!(names.contains(&"_valid".to_string()));
        assert!(names.contains(&"$also_valid".to_string()));
    }

    #[test]
    fn returns_empty_on_parse_error() {
        let src = "exports.!!! = 1;";
        let names = extract_cjs_export_names(src);
        assert_eq!(names, [] as [String; 0]);
    }

    #[test]
    fn extracts_from_conditional_branches() {
        let src = r#"
            if (process.env.NODE_ENV === "production") {
              exports.a = 1;
            } else {
              exports.b = 2;
            }
        "#;
        let names = extract_cjs_export_names(src);
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn extracts_from_try_catch() {
        let src = r"
            try {
              exports.tryName = 1;
            } catch (e) {
              exports.catchName = 2;
            } finally {
              exports.finallyName = 3;
            }
        ";
        let names = extract_cjs_export_names(src);
        assert_eq!(names, vec!["catchName", "finallyName", "tryName"]);
    }
}
