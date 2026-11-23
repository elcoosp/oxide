//! The §V2-F17 `data-ox-src` injection — the DOM→source mapping half of
//! the component-cost overlay (the server half, `GET /__oxide/cost`,
//! landed in `oxide-fw-server::cost_endpoint`): in dev, the compiler
//! stamps the first element each component renders with
//! `data-ox-src="path:line"`, so the overlay can attribute a hovered DOM
//! node to its defining component and fetch that component's cost record
//! without re-spelling the path (the value IS the `?src=` contract).
//!
//! The rule is syntactic and documented, never speculative:
//!
//! - a **component** is a top-level item the React-Refresh rule already
//!   recognizes — a function or class declaration (including their
//!   `export`-wrapped and default-export forms) or an arrow/function/
//!   class expression bound to a variable — filtered by the uppercase-
//!   first name rule the islands classifier uses. An anonymous default
//!   export has no name to filter and IS a component (the route module's
//!   root). Lowercase top-level helpers are not components — stamping
//!   them would put overlay handles on rows rendered by whatever real
//!   component lists them;
//! - the **first element** is the first `JSXElement` lexically inside the
//!   component's first JSX-bearing `return` (fragments are transparent —
//!   the fragment itself is not an element; conditional returns skip the
//!   arms that render nothing; callbacks, casts and wrappers inside the
//!   returned expression are descended). Elements not in return position
//!   are never stamped: the overlay maps what a component RENDERS, not
//!   what its event handlers build;
//! - the **line** is the 1-based line of the component's declaration —
//!   the value the overlay shows (`Card — src/islands/card.tsx:3`), and
//!   the file part is the `path` spelling the caller hands the transform
//!   (root-relative, the manifest key spelling the cost endpoint's
//!   `?src=` takes);
//! - the edit is **span surgery** on the original text (the
//!   `inject_react_refresh` emit pattern): only the attribute bytes are
//!   inserted, every other byte — and therefore every line count, so the
//!   dev sourcemap stays valid — is preserved. The attribute joins the
//!   element BEFORE the JSX lowering runs, so the runtime's own JSX pass
//!   carries it into the props call;
//! - **idempotent**: an element that already carries `data-ox-src` (hand
//!   written, or stamped by a previous pass over the same source) is
//!   skipped — the dev pipeline re-serves modules, and a second stamp on
//!   the same element would be a lie;
//! - the **no-vouch direction**: a source that does not parse, a non-JSX
//!   language, or a module with no component is returned unchanged. A
//!   skipped stamp costs the overlay nothing; a wrong one misleads it.
//!
//! Vue and Svelte are covered by the template stamp below: a Vue SFC
//! stamps the first element inside its `<template>` block, a Svelte file
//! stamps the first element of its markup region (after the `</script>`
//! close) — the same DOM→source contract, the same span-surgery rule, the
//! same idempotency guard, applied to template syntax the JSX pass cannot
//! see (Vue/Svelte compile through the Node tooling pool, but the stamp is
//! a pure-Rust pass over the source text BEFORE the pool runs, so it is
//! V8-free and testable). The stamp is dev-only: the wire-up lives in
//! [`crate::lower_to_js_with_refresh`], the dev lowering entry the ESM dev
//! server serves modules through; the production lowering
//! ([`crate::lower_to_js`]) never sees the pass.
//!
//! The module never touches the filesystem and never refuses: the dev
//! pipeline calls it per served module and serves what comes back.

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, ClassBody, ClassElement, Declaration,
    ExportDefaultDeclarationKind, Expression, Function, JSXAttributeItem, JSXAttributeName,
    JSXChild, JSXElement, JSXOpeningElement, Program, Statement, VariableDeclarator,
};
use oxc_parser::Parser;
use tracing::debug_span;

/// The DOM→source attribute the overlay reads. Public so the server's
/// cost endpoint docs and the packages' hydration tools can name the
/// attribute without a second spelling of it.
pub const DATA_OX_SRC_ATTR: &str = "data-ox-src";

/// Stamp the first rendered element of every top-level component in
/// `code` with `data-ox-src="{path}:{line}"` — the §V2-F17 DOM→source
/// mapping (module doc for the exact rule). Span surgery on the original
/// text: only the attribute bytes are inserted, every other byte (and
/// every line count, so the dev sourcemap stays valid) is preserved.
///
/// Two stamp paths, dispatched by `lang`:
///
/// - `tsx` / `jsx` — the JSX pass: every top-level component's first
///   **returned** JSX element is stamped (the React/Solid half).
/// - `vue` / `svelte` — the template pass: the first element of the
///   Vue `<template>` block or the Svelte markup region is stamped (the
///   file IS the component, so the line is `1` — the same `src` spelling
///   the cost endpoint's `?src=` contract takes, and the bundler's
///   per-file `source_costs` key).
///
/// Returns the source unchanged for: a language neither path owns, a
/// source that does not parse (the JSX half — the lowering reports the
/// parse diagnostic), and a module with no stampable element. Idempotent:
/// an element already carrying [`DATA_OX_SRC_ATTR`] is skipped, so
/// re-injecting an already-stamped source returns it unchanged.
///
/// Dev-only by construction: the wire-up is
/// [`crate::lower_to_js_with_refresh`], the dev lowering entry; the
/// production [`crate::lower_to_js`] never runs it.
///
/// # Errors
///
/// Never in the current implementation — the signature keeps `Result` so
/// a backend swap (D4) can surface parse diagnostics unchanged, the
/// [`crate::inject_react_refresh`] convention.
pub fn inject_data_ox_src(code: &str, lang: &str, path: &str) -> crate::Result<Option<String>> {
    let _span = debug_span!("inject_data_ox_src").entered();
    if matches!(lang, "vue" | "svelte") {
        return inject_template_data_ox_src(code, lang, path);
    }
    if !matches!(lang, "tsx" | "jsx") {
        return Ok(None);
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, crate::oxc_backend::source_type_for(lang)).parse();
    if !parsed.diagnostics.is_empty() {
        return Ok(None);
    }
    let mut edits = collect_component_edits(&parsed.program, code, path);
    if edits.is_empty() {
        return Ok(None);
    }
    // One insert per element, descending by position for splice safety
    // (the `inject_react_refresh` emit order).
    edits.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut out = code.to_string();
    for (at, text) in edits {
        out.insert_str(at, &text);
    }
    Ok(Some(out))
}

/// Stamp the first element of a Vue `<template>` block or a Svelte markup
/// region with `data-ox-src="{path}:1"` — the §V2-F17 DOM→source mapping
/// for the template frameworks (the file IS the component, so the line is
/// `1`, the same `src` spelling the cost endpoint's `?src=` contract
/// takes). Span surgery on the original text: only the attribute bytes are
/// inserted, every other byte is preserved. The Vue/Svelte compile hooks
/// run later through the Node tooling pool, but the stamp lands BEFORE
/// them on the source text, so the pool's output carries the attribute into
/// the rendered DOM.
///
/// The scan is a lightweight, pure-Rust pass over the template syntax (the
/// JSX AST visitor cannot see template syntax). It finds the first opening
/// tag in the template region and inserts the attribute before the tag's
/// closing `>` (or before the `/` of a self-closing tag, so the slash stays
/// attached to the angle bracket — the JSX half's `stamp_edit` rule).
///
/// Returns the source unchanged for: a Vue SFC without a `<template>`
/// block, a Svelte file whose markup region has no element, an element that
/// already carries [`DATA_OX_SRC_ATTR`] (idempotency), and a tag shape the
/// scan cannot safely attribute (a skipped stamp costs the overlay nothing,
/// a wrong one misleads it).
///
/// # Errors
///
/// Never — the template scan is infallible (a malformed template is an
/// honest no-op, the same no-vouch direction the JSX half takes).
pub fn inject_template_data_ox_src(
    code: &str,
    lang: &str,
    path: &str,
) -> crate::Result<Option<String>> {
    let _span = debug_span!("inject_template_data_ox_src").entered();
    let region = template_region(code, lang);
    let Some(region) = region else {
        return Ok(None);
    };
    let Some(tag) = first_opening_tag(&code[region.clone()]) else {
        return Ok(None);
    };
    // The full opening tag (from `<` to `>`) is the idempotency scan's
    // scope — a hand-written or previously-stamped attribute sits anywhere
    // in the tag's head. The scan walks from the tag's start to its
    // closing `>` (or `/>`), so a stamp anywhere on the element's opening
    // tag counts.
    let tag_start_in_region = tag.name_start_in_region;
    let tag_head_end = tag_start_in_region
        + opening_tag_end(&code[region.clone()][tag_start_in_region..]).unwrap_or(0);
    if carries_template_stamp(
        &code[region.start + tag_start_in_region..region.start + tag_head_end],
    ) {
        return Ok(None);
    }
    // The absolute insertion offset in the full source.
    let abs_name_end = region.start + tag.name_end_in_region;
    let Some(at) = insertion_offset(code, abs_name_end) else {
        return Ok(None);
    };
    let value = format!("{path}:1");
    let text = format!(" {DATA_OX_SRC_ATTR}=\"{value}\"");
    let mut out = code.to_string();
    out.insert_str(at, &text);
    Ok(Some(out))
}

/// The byte range of the template region the stamp scans: a Vue SFC's
/// `<template>...</template>` block (the block's inner content, after the
/// opening tag and before the closing tag), or a Svelte file's markup
/// region (everything after the last `</script>` close — the markup that
/// follows the script block(s); a file with no script block is all
/// markup).
fn template_region(code: &str, lang: &str) -> Option<std::ops::Range<usize>> {
    match lang {
        "vue" => {
            // The first top-level `<template>` block. SFCs may carry
            // `<template>` inside other blocks; the component's render
            // template is the first top-level one.
            let open = find_tag_open(code, "template")?;
            let after_open = open.end;
            let close = find_tag_close(code, "template", after_open)?;
            Some(after_open..close.start)
        }
        "svelte" => {
            // The markup region: after the last `</script>` close. A
            // Svelte file may carry a `<script>` and a `<script
            // context="module">`; the markup follows the final close. A
            // file with no script block is all markup (the region is the
            // whole file).
            let mut search_from = 0usize;
            let mut after_last_script = 0usize;
            while let Some(close) = find_tag_close(code, "script", search_from) {
                after_last_script = close.end;
                search_from = close.end;
            }
            Some(after_last_script..code.len())
        }
        _ => None,
    }
}

/// A found tag's `[start, end)` byte range — the whole `<...>` span
/// (open) or `</...>` span (close).
type TagSpan = std::ops::Range<usize>;

/// The first opening tag `name` in `code` (e.g. `<template>`, `<script>`).
/// Returns the whole tag's byte range, or `None` when absent. A naive
/// scanner is enough: the SFC's top-level tags are unambiguous, and a
/// `<` inside a script string would only confuse a tag that does not
/// appear there.
fn find_tag_open(code: &str, name: &str) -> Option<TagSpan> {
    let needle_open = format!("<{name}");
    let start = code.find(&needle_open)?;
    // The tag's close is the next `>` (SFC tags carry no `>` in attributes
    // in practice; a quoted `>` inside an attribute value is the only
    // ambiguity, and Vue SFC top-level block tags carry none).
    let after = start + needle_open.len();
    let gt = code[after..].find('>')?;
    let end = after + gt + 1;
    Some(start..end)
}

/// The first closing tag `</name>` at or after `from` in `code`. Returns
/// the whole `</...>` byte range, or `None` when absent.
fn find_tag_close(code: &str, name: &str, from: usize) -> Option<TagSpan> {
    let needle_close = format!("</{name}");
    let start = code[from..].find(&needle_close)? + from;
    let after = start + needle_close.len();
    let gt = code[after..].find('>')?;
    let end = after + gt + 1;
    Some(start..end)
}

/// The first opening element tag inside the template region: the tag's
/// name span (the byte offset in the region where the tag name ends — the
/// insertion point for the attribute lands just after the name, before any
/// existing attributes or the closing `>`/`/>`). A `<` that is not
/// followed by a tag-name char (a `</`, a `<!--` comment, or a `<` text
/// node) is skipped; the first real element wins.
struct FirstTag {
    /// The byte offset (relative to the region's start) where the tag
    /// starts (`<`).
    name_start_in_region: usize,
    /// The byte offset (relative to the region's start) where the tag
    /// name ends — the `<tagname` span's end, before any whitespace or
    /// attribute.
    name_end_in_region: usize,
}

/// Scan the region for the first opening element tag and return where its
/// name ends (relative to the region). Comments, closing tags and bare
/// text are skipped. Self-closing and attribute-bearing tags are handled by
/// the caller's `insertion_offset`.
fn first_opening_tag(region: &str) -> Option<FirstTag> {
    let bytes = region.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // A `<` — what follows?
        let next = bytes.get(i + 1).copied()?;
        // `</` is a closing tag — skip to its `>`.
        if next == b'/' {
            let gt = region[i + 2..].find('>')?;
            i = i + 2 + gt + 1;
            continue;
        }
        // `<!--` is a comment — skip to `-->`.
        if region[i..].starts_with("<!--") {
            let end = region[i + 4..].find("-->")?;
            i = i + 4 + end + 3;
            continue;
        }
        // `<!` (doctype/declaration) — skip to `>`.
        if next == b'!' {
            let gt = region[i + 2..].find('>')?;
            i = i + 2 + gt + 1;
            continue;
        }
        // A tag name char (a letter) — this is an opening element.
        if next.is_ascii_alphabetic() {
            // The name runs until a non-name char (whitespace, `/`, `>`).
            let mut j = i + 1;
            while j < bytes.len() {
                let c = bytes[j];
                if c.is_ascii_alphanumeric() || c == b'-' || c == b':' || c == b'_' || c == b'.' {
                    j += 1;
                } else {
                    break;
                }
            }
            return Some(FirstTag {
                name_start_in_region: i,
                name_end_in_region: j,
            });
        }
        // A bare `<` followed by something else — skip the char.
        i += 1;
    }
    None
}

/// The length (in bytes) of the opening tag's head — from the leading `<`
/// to and including the closing `>` or `/>`. Used by the idempotency scan
/// to bound the attribute search. Returns `None` for a tag with no closing
/// `>` (an honest no-op the caller treats as zero).
fn opening_tag_end(tag: &str) -> Option<usize> {
    let bytes = tag.as_bytes();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'>' if !in_single && !in_double => return Some(i + 1),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether the opening tag's head already carries the stamp attribute —
/// the idempotency guard (a hand-written stamp or a previous pass's stamp
/// means the element is already attributed, so a second injection is a
/// no-op). The scan covers the whole tag head (from `<` to `>`).
fn carries_template_stamp(tag_head: &str) -> bool {
    tag_head.contains(DATA_OX_SRC_ATTR)
}

/// The byte offset (in the full source) where the attribute text goes:
/// just before the tag's closing `>` (or before the `/` of a self-closing
/// tag, so the slash stays attached to the angle bracket — the JSX half's
/// `stamp_edit` rule). `name_end` is the absolute offset where the tag
/// name ends; the scan walks forward from there to find the closing `>` or
/// `/>`, skipping attributes and whitespace. Returns `None` for a tag shape
/// the scan cannot safely attribute (no closing `>`, or a `>` inside a
/// quoted attribute value it cannot resolve — an honest no-op).
fn insertion_offset(code: &str, name_end: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut i = name_end;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            // A `/` outside a quoted value is the self-closing slash (the
            // `/>` shape); a `>` is the tag's close. Either is the
            // insertion point — insert before it (the JSX half's
            // `stamp_edit` rule).
            b'/' | b'>' if !in_single && !in_double => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// One attribute insertion: the byte offset the attribute text goes
/// before, and the text (` data-ox-src="path:line"` — the leading space
/// separates it from the tag's own attributes).
struct StampEdit {
    at: usize,
    text: String,
}

/// Where a component's first returned element lives: the statements of a
/// function body (declarations and bound expressions), an arrow's
/// implicit-return expression, or a class's body.
enum ComponentSource<'a> {
    Statements(&'a [Statement<'a>]),
    ImplicitReturn(&'a Expression<'a>),
    ClassBody(&'a ClassBody<'a>),
}

/// A top-level component candidate: where it is declared (for the line)
/// and where its first rendered element is looked up.
struct ComponentCandidate<'a> {
    decl_start: u32,
    source: ComponentSource<'a>,
}

/// Walk the program's top-level statements and collect every component
/// candidate (the module doc's rule). Mirrors `oxc_backend`'s
/// `top_level_component_names` statement table — the two surfaces must
/// recognize the same components — while keeping the body and the
/// declaration position the name table does not need.
fn collect_component_candidates<'a>(program: &'a Program<'a>) -> Vec<ComponentCandidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::FunctionDeclaration(f) => {
                if let Some(source) = fn_body_source(f) {
                    push_if_component(
                        f.id.as_ref().map_or("", |id| id.name.as_str()),
                        f.span.start,
                        source,
                        &mut out,
                    );
                }
            }
            Statement::ClassDeclaration(c) => {
                push_if_component(
                    c.id.as_ref().map_or("", |id| id.name.as_str()),
                    c.span.start,
                    ComponentSource::ClassBody(&c.body),
                    &mut out,
                );
            }
            Statement::VariableDeclaration(v) => {
                for decl in &v.declarations {
                    push_bound_component(decl, &mut out);
                }
            }
            Statement::ExportDeclaration(e) => match &e.declaration {
                Declaration::FunctionDeclaration(f) => {
                    if let Some(source) = fn_body_source(f) {
                        push_if_component(
                            f.id.as_ref().map_or("", |id| id.name.as_str()),
                            f.span.start,
                            source,
                            &mut out,
                        );
                    }
                }
                Declaration::ClassDeclaration(c) => {
                    push_if_component(
                        c.id.as_ref().map_or("", |id| id.name.as_str()),
                        c.span.start,
                        ComponentSource::ClassBody(&c.body),
                        &mut out,
                    );
                }
                Declaration::VariableDeclaration(v) => {
                    for decl in &v.declarations {
                        push_bound_component(decl, &mut out);
                    }
                }
                _ => {}
            },
            Statement::ExportDefaultDeclaration(e) => match &e.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                    // A default export is the module's root component
                    // whether or not it carries a name (the anonymous
                    // route-module form) — no name filter here.
                    if let Some(source) = fn_body_source(f) {
                        out.push(ComponentCandidate {
                            decl_start: f.span.start,
                            source,
                        });
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(c)
                | ExportDefaultDeclarationKind::ClassExpression(c) => {
                    out.push(ComponentCandidate {
                        decl_start: c.span.start,
                        source: ComponentSource::ClassBody(&c.body),
                    });
                }
                ExportDefaultDeclarationKind::ArrowFunctionExpression(a) => {
                    out.push(ComponentCandidate {
                        decl_start: a.span.start,
                        source: arrow_source(a),
                    });
                }
                ExportDefaultDeclarationKind::FunctionExpression(f) => {
                    if let Some(source) = fn_body_source(f) {
                        out.push(ComponentCandidate {
                            decl_start: f.span.start,
                            source,
                        });
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    out
}

/// A function's body source (declarations, expressions, methods). The
/// name filter is the caller's decision — only anonymous default exports
/// skip it — the body extraction is shared.
fn fn_body_source<'a>(f: &'a Function<'a>) -> Option<ComponentSource<'a>> {
    f.body
        .as_deref()
        .map(|body| ComponentSource::Statements(&body.statements))
}

/// The component-name rule: an uppercase-first identifier is a component
/// (the islands classifier's rule, and the React convention the adapters
/// generate).
fn is_component_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Push a named declaration candidate when its name passes the component
/// rule. `decl_start` is the component value's own span (the `function`
/// keyword or the bound expression), not the wrapping `export`.
fn push_if_component<'a>(
    name: &str,
    decl_start: u32,
    source: ComponentSource<'a>,
    out: &mut Vec<ComponentCandidate<'a>>,
) {
    if is_component_name(name) {
        out.push(ComponentCandidate { decl_start, source });
    }
}

/// The variable-bound form: the initializer must be a function or class
/// expression (the refresh rule), the binding name must pass the
/// component rule.
fn push_bound_component<'a>(
    decl: &'a VariableDeclarator<'a>,
    out: &mut Vec<ComponentCandidate<'a>>,
) {
    let Some(init) = decl.init.as_ref() else {
        return;
    };
    let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id else {
        return;
    };
    let (decl_start, source) = match init {
        Expression::ArrowFunctionExpression(a) => (a.span.start, arrow_source(a)),
        Expression::FunctionExpression(f) => match f.body.as_deref() {
            Some(body) => (f.span.start, ComponentSource::Statements(&body.statements)),
            None => return,
        },
        Expression::ClassExpression(c) => (c.span.start, ComponentSource::ClassBody(&c.body)),
        _ => return,
    };
    push_if_component(id.name.as_str(), decl_start, source, out);
}

/// An arrow's body: a block body walks as statements; an implicit-return
/// expression is the first element's search space directly.
fn arrow_source<'a>(arrow: &'a oxc_ast::ast::ArrowFunctionExpression<'a>) -> ComponentSource<'a> {
    match &arrow.body {
        ArrowFunctionBody::FunctionBody(body) => ComponentSource::Statements(&body.statements),
        // Every non-FunctionBody variant is an inherited expression.
        other => match other.as_expression() {
            Some(expr) => ComponentSource::ImplicitReturn(expr),
            // Unreachable today; the empty arm keeps the match total.
            None => ComponentSource::Statements(&[]),
        },
    }
}

/// Collect the stamp edits for every candidate: find its first returned
/// element, skip the ones already attributed, compute the insertion
/// points. Two candidates can never share an element (top-level bodies
/// are disjoint spans), so positions are unique.
fn collect_component_edits(
    program: &Program<'_>,
    source: &str,
    path: &str,
) -> Vec<(usize, String)> {
    let mut edits: Vec<(usize, String)> = Vec::new();
    for candidate in collect_component_candidates(program) {
        let element = match &candidate.source {
            ComponentSource::Statements(stmts) => first_returned_element(stmts),
            ComponentSource::ImplicitReturn(expr) => first_element_in_expression(expr),
            ComponentSource::ClassBody(body) => first_element_in_class(body),
        };
        let Some(element) = element else {
            continue;
        };
        let opening = &element.opening_element;
        if carries_stamp(opening) {
            continue;
        }
        let value = format!("{path}:{}", line_of(source, candidate.decl_start));
        let Some(edit) = stamp_edit(opening, source, &value) else {
            continue;
        };
        edits.push((edit.at, edit.text));
    }
    edits
}

/// The insertion point and text for one opening element: just before the
/// tag's closing `>` (or before the `/` of a self-closing tag, so the
/// slash stays attached to the angle bracket). Defensive about the tag
/// text's tail — a shape oxc does not document today skips the stamp
/// instead of corrupting the source.
fn stamp_edit(opening: &JSXOpeningElement<'_>, source: &str, value: &str) -> Option<StampEdit> {
    let end = usize::try_from(opening.span.end).ok()?;
    let before = source.get(..end)?;
    let at = if before.ends_with("/>") {
        end - 2
    } else if before.ends_with('>') {
        end - 1
    } else {
        return None;
    };
    Some(StampEdit {
        at,
        text: format!(" {DATA_OX_SRC_ATTR}=\"{value}\""),
    })
}

/// Whether the opening element already carries the stamp attribute (the
/// idempotency rule).
fn carries_stamp(opening: &JSXOpeningElement<'_>) -> bool {
    opening.attributes.iter().any(|item| {
        matches!(
            item,
            JSXAttributeItem::Attribute(attr)
                if matches!(&attr.name, JSXAttributeName::Identifier(name) if name.name.as_str() == DATA_OX_SRC_ATTR)
        )
    })
}

/// The 1-based line of a byte offset (the enrich arithmetic the surfaces
/// share; a multi-byte character does not shift the count).
fn line_of(source: &str, offset: u32) -> u32 {
    let start = usize::try_from(offset).map_or(source.len(), |at| at.min(source.len()));
    1 + u32::try_from(source[..start].bytes().filter(|b| *b == b'\n').count())
        .unwrap_or(u32::MAX - 1)
}

/// The first JSX element inside a component's statements: the first
/// JSX-bearing `return` wins (an earlier `return null` skips to the next
/// statement), descending through the statement containers a render path
/// uses (blocks, conditionals, try arms, loops, switches, labels).
fn first_returned_element<'a>(stmts: &'a [Statement<'a>]) -> Option<&'a JSXElement<'a>> {
    stmts.iter().find_map(first_element_in_statement)
}

/// One statement's JSX-bearing return, or the descent into its
/// containers.
fn first_element_in_statement<'a>(stmt: &'a Statement<'a>) -> Option<&'a JSXElement<'a>> {
    match stmt {
        Statement::ReturnStatement(ret) => {
            ret.argument.as_ref().and_then(first_element_in_expression)
        }
        Statement::BlockStatement(block) => first_returned_element(&block.body),
        Statement::IfStatement(i) => first_element_in_statement(&i.consequent)
            .or(i.alternate.as_ref().and_then(first_element_in_statement)),
        Statement::TryStatement(t) => first_returned_element(&t.block.body)
            .or(t
                .handler
                .as_ref()
                .and_then(|h| first_returned_element(&h.body.body)))
            .or(t
                .finalizer
                .as_ref()
                .and_then(|f| first_returned_element(&f.body))),
        Statement::SwitchStatement(s) => s
            .cases
            .iter()
            .find_map(|c| first_returned_element(&c.consequent)),
        Statement::ForStatement(f) => first_element_in_statement(&f.body),
        Statement::ForInStatement(f) => first_element_in_statement(&f.body),
        Statement::ForOfStatement(f) => first_element_in_statement(&f.body),
        Statement::WhileStatement(w) => first_element_in_statement(&w.body),
        Statement::DoWhileStatement(d) => first_element_in_statement(&d.body),
        Statement::LabeledStatement(l) => first_element_in_statement(&l.body),
        _ => None,
    }
}

/// A class component renders from its method bodies (the `render` method
/// in React, the class form of a Solid/Svelte component): walk the
/// elements in declaration order, the first JSX-bearing member wins.
fn first_element_in_class<'a>(body: &'a ClassBody<'a>) -> Option<&'a JSXElement<'a>> {
    body.body.iter().find_map(|element| match element {
        ClassElement::MethodDefinition(method) => method
            .value
            .body
            .as_deref()
            .and_then(|body| first_returned_element(&body.statements)),
        ClassElement::PropertyDefinition(prop) => {
            prop.value.as_ref().and_then(first_element_in_expression)
        }
        _ => None,
    })
}

/// The first JSX element lexically inside a returned expression, in
/// pre-order: the element itself stops the walk; fragments are
/// transparent (their first element child is the answer); wrappers
/// (await, TS casts, logical and conditional arms, call/new arguments,
/// array elements, object values, callback bodies) are descended.
fn first_element_in_expression<'a>(expr: &'a Expression<'a>) -> Option<&'a JSXElement<'a>> {
    match expr {
        Expression::JSXElement(el) => Some(el.as_ref()),
        Expression::JSXFragment(frag) => frag.children.iter().find_map(|child| match child {
            JSXChild::Element(el) => Some(el.as_ref()),
            JSXChild::Fragment(frag) => frag.children.iter().find_map(|inner| match inner {
                JSXChild::Element(el) => Some(el.as_ref()),
                _ => None,
            }),
            JSXChild::ExpressionContainer(container) => container
                .expression
                .as_expression()
                .and_then(first_element_in_expression),
            _ => None,
        }),
        Expression::ConditionalExpression(c) => {
            first_element_in_expression(&c.consequent).or(first_element_in_expression(&c.alternate))
        }
        // The parser preserves parentheses by default; a parenthesized
        // return argument is the common multi-line render shape.
        Expression::ParenthesizedExpression(p) => first_element_in_expression(&p.expression),
        Expression::LogicalExpression(l) => {
            first_element_in_expression(&l.left).or(first_element_in_expression(&l.right))
        }
        Expression::AwaitExpression(a) => first_element_in_expression(&a.argument),
        Expression::TSAsExpression(t) => first_element_in_expression(&t.expression),
        Expression::TSSatisfiesExpression(t) => first_element_in_expression(&t.expression),
        Expression::TSNonNullExpression(t) => first_element_in_expression(&t.expression),
        Expression::ArrowFunctionExpression(a) => match &a.body {
            ArrowFunctionBody::FunctionBody(body) => first_returned_element(&body.statements),
            // Every non-FunctionBody variant is an inherited expression.
            other => other.as_expression().and_then(first_element_in_expression),
        },
        Expression::FunctionExpression(f) => f
            .body
            .as_deref()
            .and_then(|body| first_returned_element(&body.statements)),
        Expression::ArrayExpression(arr) => arr.elements.iter().find_map(|el| match el {
            ArrayExpressionElement::SpreadElement(spread) => {
                first_element_in_expression(&spread.argument)
            }
            _ => el.as_expression().and_then(first_element_in_expression),
        }),
        Expression::ObjectExpression(obj) => obj.properties.iter().find_map(|prop| match prop {
            oxc_ast::ast::ObjectPropertyKind::ObjectProperty(prop) => {
                first_element_in_expression(&prop.value)
            }
            // A spread in an object literal carries no rendered element.
            oxc_ast::ast::ObjectPropertyKind::SpreadProperty(_) => None,
        }),
        // The JSX inside a `React.createElement(...)` or `items.map(...)`
        // call argument is what the call renders.
        Expression::CallExpression(call) => call.arguments.iter().find_map(|arg| match arg {
            Argument::SpreadElement(spread) => first_element_in_expression(&spread.argument),
            _ => arg.as_expression().and_then(first_element_in_expression),
        }),
        Expression::NewExpression(new) => new.arguments.iter().find_map(|arg| match arg {
            Argument::SpreadElement(spread) => first_element_in_expression(&spread.argument),
            _ => arg.as_expression().and_then(first_element_in_expression),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dev pipeline's pipeline arm rides the walk's honesty: a plain
    /// React-shaped module stamps exactly once, at the declaration line.
    #[test]
    fn module_pin_route_module_stamps_once() {
        let src = "export default function Page() {\n  return <main>hi</main>;\n}\n";
        let out = inject_data_ox_src(src, "tsx", "src/routes/page.tsx")
            .unwrap()
            .unwrap();
        assert_eq!(
            out.matches(DATA_OX_SRC_ATTR).count(),
            1,
            "one component, one stamp: {out}"
        );
        assert!(out.contains("src/routes/page.tsx:1"), "{out}");
    }

    /// The module pin for the no-vouch direction: an unparseable source
    /// is returned untouched (the lowering reports the diagnostic).
    #[test]
    fn module_pin_unparseable_is_untouched() {
        let src = "function broken( { return <div></div }";
        assert_eq!(inject_data_ox_src(src, "tsx", "src/b.tsx").unwrap(), None);
    }

    /// Walk probe: what does the fragment source produce?
    #[test]
    fn probe_fragment_walk() {
        let src = "function Page() {\n  return (\n    <>\n      <section id=\"a\" />\n      <p>after</p>\n    </>\n  );\n}\n";
        let out = inject_data_ox_src(src, "tsx", "src/a.tsx")
            .unwrap()
            .unwrap();
        assert!(out.contains("data-ox-src=\"src/a.tsx:1\""), "{out}");
        assert!(!out.contains("<p data-ox-src"), "{out}");
    }
}
