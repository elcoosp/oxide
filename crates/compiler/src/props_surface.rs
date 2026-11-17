//! The §V2-F02 dev warn — the props-surface half of the auto-islands
//! inference: which components receive `on*` props at their usage sites,
//! and which of those defining files the [`IslandManifest`] classified
//! **pure** (the only classification the hydration decision skips).
//!
//! The build scan (the [`crate::interactivity`] engine + the CLI
//! adoption) decides what hydrates. A pure classification is a guess, and
//! the guess has one blind spot the scanner cannot see from inside the
//! defining file: a parent wiring `onClick` through props into a
//! component the classifier called pure. Either the props are dead (the
//! handlers never attach) or the component forwards them to a DOM element
//! and only hydrates by luck. The dev-time warn names that mismatch where
//! the developer can act on it — at the transform pipeline, when the
//! module is served.
//!
//! The surface is syntactic and honest, never conservative:
//!
//! - a JSX component element (uppercase-first identifier base, or the
//!   base of a member name) carrying `on*` attributes is a usage — the
//!   2-char prefix rule is the classifier's own ([`scan_on_props`]
//!   mirrors `visit_jsx_attribute_item`'s predicate verbatim, so the two
//!   surfaces can never disagree about what `on*` means);
//! - host elements are not usages, spreads claim nothing, a source that
//!   does not parse reports nothing — a skipped warn costs nothing, a
//!   false one trains the developer to ignore the channel;
//! - the warn fires ONLY on a positive pure classification. A defining
//!   file the manifest classifies interactive (including a
//!   fixpoint-promoted one), does not know (not scanned — the
//!   [`crate::interactivity::hydrates`] missing-data rule), or that is
//!   the usage file itself (carrying the `on*` attribute makes the file
//!   interactive by its own scan) never warns. Package specifiers carry
//!   no same-project evidence (the fixpoint's rule, shared here).
//!
//! The module never touches the filesystem and never refuses: the dev
//! pipeline calls it per served module and logs what comes back.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    ImportDeclaration, ImportDeclarationSpecifier, JSXAttributeItem, JSXAttributeName,
    JSXElementName, JSXMemberExpressionObject, JSXOpeningElement,
};
use oxc_ast_visit::Visit;
use oxc_ast_visit::walk::{walk_import_declaration, walk_jsx_opening_element};
use serde::{Deserialize, Serialize};

use crate::framework::Framework;
use crate::interactivity::{IslandManifest, relative_join, specifier_candidates};

/// One `on*`-props usage site: a JSX component element, the event-shaped
/// props it receives (sorted, deduped), and where the opening tag sits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnPropsUsage {
    /// The JSX base name the usage references: `Card` for `<Card …>`,
    /// `Icons` for `<Icons.Cog …>` (the binding the import map resolves —
    /// the member property is the namespace's export, not the binding).
    pub component: String,
    /// The `on*` attribute names, sorted, deduped.
    pub props: Vec<String>,
    /// 1-based line of the opening tag.
    pub line: u32,
    /// The trimmed source line containing the opening tag.
    pub snippet: String,
}

/// One warn: a pure-classified defining file receiving `on*` props at a
/// usage site. Every field the dev pipeline's `tracing` event logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PureOnPropsWarn {
    /// The file carrying the usage site (manifest-key spelling).
    pub usage_file: String,
    /// The JSX base name the usage references.
    pub component: String,
    /// The defining file the specifier resolved to — the file the
    /// manifest classified pure.
    pub defining_file: String,
    /// The `on*` attribute names, sorted, deduped.
    pub props: Vec<String>,
    /// 1-based line of the opening tag in the usage file.
    pub line: u32,
    /// The trimmed usage-site source line.
    pub snippet: String,
}

/// The raw hit a visitor records, before line/snippet enrichment.
struct RawUsage {
    span: (usize, usize),
    component: String,
    props: Vec<String>,
}

/// The scan state for one module: usage hits + import bindings, collected
/// in one parse (the dev pipeline pays the Oxc parse once per served
/// file).
struct SurfaceVisitor {
    usages: Vec<RawUsage>,
    imports: BTreeMap<String, String>,
}

/// The base binding name a JSX element name references: the identifier
/// itself, or the leftmost object of a member chain; `<this.x>` and
/// namespaced names reference no importable binding.
fn base_name<'a>(name: &'a JSXElementName<'a>) -> Option<&'a str> {
    match name {
        JSXElementName::Identifier(id) => Some(id.name.as_str()),
        JSXElementName::IdentifierReference(id) => Some(id.name.as_str()),
        JSXElementName::MemberExpression(member) => base_object(&member.object),
        JSXElementName::NamespacedName(_) | JSXElementName::ThisExpression(_) => None,
    }
}

/// The leftmost binding of a member-object chain (`Icons` of
/// `<Icons.Cog.Pog />`).
fn base_object<'a>(object: &'a JSXMemberExpressionObject<'a>) -> Option<&'a str> {
    match object {
        JSXMemberExpressionObject::IdentifierReference(id) => Some(id.name.as_str()),
        JSXMemberExpressionObject::MemberExpression(member) => base_object(&member.object),
        JSXMemberExpressionObject::ThisExpression(_) => None,
    }
}

/// The JSX component-name rule: an uppercase-first ASCII base is a
/// component; a lowercase base is a host element. (`_`-prefixed local
/// components are rare and land on the no-usage side — the surface is an
/// opinion, and this one matches the framework conventions the adapters
/// generate.)
fn is_component_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// The classifier's own `on*` attribute rule, mirrored verbatim from
/// `interactivity.rs`'s `visit_jsx_attribute_item`: ANY attribute whose
/// name starts with a case-insensitive `on`. A stray `only` lands too —
/// the two surfaces must never disagree about what `on*` means.
fn on_prop_name(item: &JSXAttributeItem<'_>) -> Option<String> {
    if let JSXAttributeItem::Attribute(attr) = item
        && let JSXAttributeName::Identifier(name) = &attr.name
        && name
            .name
            .as_str()
            .get(..2)
            .is_some_and(|p| p.eq_ignore_ascii_case("on"))
    {
        return Some(name.name.to_string());
    }
    None
}

impl<'a> Visit<'a> for SurfaceVisitor {
    fn visit_jsx_opening_element(&mut self, it: &JSXOpeningElement<'a>) {
        if let Some(base) = base_name(&it.name)
            && is_component_name(base)
        {
            let mut props: Vec<String> = it.attributes.iter().filter_map(on_prop_name).collect();
            props.sort();
            props.dedup();
            if !props.is_empty() {
                self.usages.push(RawUsage {
                    span: (it.span.start as usize, it.span.end as usize),
                    component: base.to_string(),
                    props,
                });
            }
        }
        walk_jsx_opening_element(self, it);
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if let Some(specifiers) = &it.specifiers {
            for spec in specifiers {
                let local = match spec {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => s.local.name.as_str(),
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => s.local.name.as_str(),
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        s.local.name.as_str()
                    }
                };
                // A lone-surrogate source is not a resolvable specifier —
                // the empty spelling resolves to nothing same-project.
                let source = it.source.value.as_str().unwrap_or_default();
                self.imports.insert(local.to_string(), source.to_string());
            }
        }
        walk_import_declaration(self, it);
    }
}

/// Parse `source` once and collect the whole surface. `false` when no
/// candidate source type parses (the surface then reports nothing — the
/// documented no-vouch direction).
fn scan_surface(source: &str) -> Option<SurfaceVisitor> {
    let allocator = oxc_allocator::Allocator::default();
    for source_type in [oxc_span::SourceType::tsx(), oxc_span::SourceType::ts()] {
        let parsed = oxc_parser::Parser::new(&allocator, source, source_type).parse();
        if parsed.diagnostics.is_empty() {
            let mut visitor = SurfaceVisitor {
                usages: Vec::new(),
                imports: BTreeMap::new(),
            };
            visitor.visit_program(&parsed.program);
            return Some(visitor);
        }
    }
    None
}

/// Enrich one raw usage with its 1-based line and trimmed source line
/// (the interactivity scanner's `enrich` arithmetic).
fn enrich(source: &str, usage: &RawUsage) -> OnPropsUsage {
    let start = usage.span.0;
    let prefix = source.get(..start).unwrap_or("");
    let line = 1u32
        + prefix
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            .try_into()
            .unwrap_or(u32::MAX - 1);
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let rest = source.get(start..).unwrap_or("");
    let line_end = rest
        .find('\n')
        .map_or(source.len(), |i| start.saturating_add(i));
    OnPropsUsage {
        component: usage.component.clone(),
        props: usage.props.clone(),
        line,
        snippet: source
            .get(line_start..line_end)
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

/// The per-component `on*`-props usage sites in one module, in pre-order.
/// A source that does not parse or carries no JSX reports nothing (the
/// module doc's no-vouch direction — a warn is an opinion, not a skip
/// decision, so the surface never invents one).
#[must_use]
pub fn scan_on_props(source: &str) -> Vec<OnPropsUsage> {
    let _span = tracing::debug_span!("scan_on_props").entered();
    match scan_surface(source) {
        Some(visitor) => visitor
            .usages
            .iter()
            .map(|usage| enrich(source, usage))
            .collect(),
        None => Vec::new(),
    }
}

/// The import bindings a JSX usage can reference in one module: local
/// binding name → specifier (default, named — aliased or not — and
/// namespace). Package specifiers land too; the resolvers below decide
/// which specifiers carry same-project evidence.
#[must_use]
pub fn scan_component_imports(source: &str) -> BTreeMap<String, String> {
    match scan_surface(source) {
        Some(visitor) => visitor.imports,
        None => BTreeMap::new(),
    }
}

/// Resolve one relative specifier from `usage_file` against the manifest
/// key set (the fixpoint's candidate rule, shared via
/// `interactivity::specifier_candidates`). Returns the manifest KEY of
/// the defining file, or `None` for package specifiers and specifiers
/// naming no scanned file.
fn resolve_in_manifest(usage_file: &str, spec: &str, manifest: &IslandManifest) -> Option<String> {
    let joined = relative_join(usage_file, spec)?;
    specifier_candidates(&joined)
        .into_iter()
        .find(|candidate| manifest.get(candidate).is_some())
}

/// The per-file check the dev pipeline runs when it serves a module: the
/// `on*`-props usage sites in `usage_source`, resolved through the file's
/// import bindings onto the manifest's defining files, warned only when a
/// defining file carries a POSITIVE pure classification. The usage scan
/// reads the current source (a fresh edit is caught on the next serve);
/// the classification is the manifest's (the dev boot's scan — a
/// mid-session change to a defining file refreshes on the next boot; the
/// delta is recorded in `docs/deviations.md`).
#[must_use]
pub fn pure_on_props_in_file(
    usage_file: &str,
    usage_source: &str,
    manifest: &IslandManifest,
) -> Vec<PureOnPropsWarn> {
    let Some(visitor) = scan_surface(usage_source) else {
        return Vec::new();
    };
    let mut warns: Vec<PureOnPropsWarn> = visitor
        .usages
        .iter()
        .filter_map(|usage| {
            let spec = visitor.imports.get(&usage.component)?;
            let defining = resolve_in_manifest(usage_file, spec, manifest)?;
            // A same-file usage is a contradiction: carrying the `on*`
            // attribute makes the file interactive by its own scan.
            if defining == usage_file {
                return None;
            }
            let entry = manifest.get(&defining)?;
            if entry.is_interactive() {
                return None;
            }
            let enriched = enrich(usage_source, usage);
            Some(PureOnPropsWarn {
                usage_file: usage_file.to_string(),
                component: enriched.component,
                defining_file: defining,
                props: enriched.props,
                line: enriched.line,
                snippet: enriched.snippet,
            })
        })
        .collect();
    warns.sort_by(|a, b| (&a.component, a.line).cmp(&(&b.component, b.line)));
    warns
}

/// The whole-project pass: every file's usage sites crossed with the
/// manifest built over the same set, sorted deterministically by
/// (usage file, line, component). The dev boot calls this shape when it
/// wants the full warn list up front; the served-module path calls
/// [`pure_on_props_in_file`] per file instead (the same contract, one
/// parse).
#[must_use]
pub fn pure_on_props_warnings(
    files: &BTreeMap<String, (Framework, String)>,
    manifest: &IslandManifest,
) -> Vec<PureOnPropsWarn> {
    let _span = tracing::debug_span!("pure_on_props_warnings").entered();
    let mut warns: Vec<PureOnPropsWarn> = files
        .iter()
        .flat_map(|(file, (_, source))| pure_on_props_in_file(file, source, manifest))
        .collect();
    warns.sort_by(|a, b| {
        (&a.usage_file, a.line, &a.component).cmp(&(&b.usage_file, b.line, &b.component))
    });
    warns
}
