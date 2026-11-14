//! The §V2-F02 auto-islands inference — the pure compiler-side
//! interactivity scanner the `.oxide/island-manifest.json` side-car and
//! the auto hydration set come from.
//!
//! The spec's one-week-spike design, landed as the V8-free substrate:
//! [`scan_interactivity`] runs **direct detection only** over one
//! component module — a real Oxc parse for the script arms (the F16
//! `infer_reasons` precedent: comments and strings are never hits), a
//! conservative token scan for the SFC template arms (no Vue/Svelte
//! template parser exists in the Rust tree; the token scan can only
//! over-flag, and an over-flagged island costs bytes while a missed one
//! silently breaks interactivity — the spike's kill criterion). The
//! project-level fixpoint ([`IslandManifest::build`]) then promotes every
//! file that imports an interactive same-project component, transitively,
//! through relative specifiers only (package imports carry no evidence).
//!
//! The conservative arms all point the same direction:
//!
//! - a module that does not parse classifies **interactive** (`unparsed`),
//! - missing manifest data hydrates ([`hydrates`]),
//! - an explicit registry entry always wins over a pure classification.
//!
//! A pure classification is the ONLY reason to skip hydration, so the
//! worst case of a wrong inference is today's behavior, never a broken
//! page. The module never touches the filesystem (the F04 ruling: the
//! side-car write and the registry merge stay at the CLI call site).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use oxc_ast::ast::{
    AssignmentTarget, CallExpression, Expression, JSXAttributeItem, JSXAttributeName,
};
use oxc_ast_visit::Visit;
use oxc_ast_visit::walk::{
    walk_assignment_expression, walk_call_expression, walk_jsx_attribute_item,
    walk_labeled_statement, walk_variable_declarator,
};
use serde::{Deserialize, Serialize};

use crate::framework::Framework;
use crate::scan_import_spans;

/// One interactivity hit: the detector that fired, its 1-based line and
/// the trimmed source line (the F16 [`crate::DynamicReason`] shape, minus
/// the span the hydration decision has no use for).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractivityReason {
    /// The detector that fired: the hook/primitive name for call arms
    /// (`useState`, `createSignal`, ...), `on-attribute` for a JSX event
    /// attribute, `imports-interactive` for a fixpoint promotion,
    /// `unparsed` for the conservative parse-failure arm.
    pub kind: String,
    /// 1-based line of the hit.
    pub line: u32,
    /// The trimmed source line containing the hit.
    pub snippet: String,
}

/// The §V2-F02 scan result (and the manifest's per-file entry): whether
/// the component is interactive and the reasons the scanner found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Interactivity {
    /// `true` when the scanner found a hit, the file imports an
    /// interactive same-project component, or the file did not parse.
    pub interactive: bool,
    /// The hits, in pre-order.
    pub reasons: Vec<InteractivityReason>,
}

impl Interactivity {
    /// The no-hits classification (the scanner's pure answer).
    #[must_use]
    pub fn pure() -> Self {
        Self {
            interactive: false,
            reasons: Vec::new(),
        }
    }

    /// The classification the hydration decision consults.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }
}

/// A hit found by the AST walk, before line/snippet enrichment. Spans
/// are whole-file byte offsets.
struct RawHit {
    span: (usize, usize),
    kind: &'static str,
}

/// The per-framework call arms: identifier (or `.property`) callees that
/// count, verbatim from the spec's token lists.
const REACT_CALLS: &[&str] = &["useState", "useReducer", "useEffect", "useLayoutEffect"];
const SOLID_CALLS: &[&str] = &["createSignal", "createEffect", "createStore"];
/// The Vue script arms — the spec's tokens plus `defineEmits`/`emit`
/// (an emitting component is interactive by construction; the delta is
/// recorded in `docs/deviations.md`).
const VUE_CALLS: &[&str] = &[
    "ref",
    "reactive",
    "computed",
    "watch",
    "onMounted",
    "defineEmits",
];
/// The Svelte script arms — the spec's `$state` rune plus `$effect`
/// (an effecting component is interactive; the delta is recorded).
const SVELTE_CALLS: &[&str] = &["$state", "$effect"];

/// The scanner state for one module: the active arms and the hits.
struct Scanner {
    calls: &'static [&'static str],
    /// `useRef`-declared identifiers (React only): the `.current`-write
    /// arm consults this set.
    refs: BTreeSet<String>,
    jsx_on: bool,
    labeled_reactive: bool,
    emit_arms: bool,
    hits: Vec<RawHit>,
}

impl Scanner {
    fn react() -> Self {
        Self {
            calls: REACT_CALLS,
            refs: BTreeSet::new(),
            jsx_on: true,
            labeled_reactive: false,
            emit_arms: false,
            hits: Vec::new(),
        }
    }

    fn solid() -> Self {
        Self {
            calls: SOLID_CALLS,
            refs: BTreeSet::new(),
            jsx_on: true,
            labeled_reactive: false,
            emit_arms: false,
            hits: Vec::new(),
        }
    }

    fn svelte_script() -> Self {
        Self {
            calls: SVELTE_CALLS,
            refs: BTreeSet::new(),
            jsx_on: false,
            labeled_reactive: true,
            emit_arms: false,
            hits: Vec::new(),
        }
    }

    fn vue_script() -> Self {
        Self {
            calls: VUE_CALLS,
            refs: BTreeSet::new(),
            jsx_on: false,
            labeled_reactive: false,
            emit_arms: true,
            hits: Vec::new(),
        }
    }

    fn hit(&mut self, span: oxc_span::Span, kind: &'static str) {
        self.hits.push(RawHit {
            span: (span.start as usize, span.end as usize),
            kind,
        });
    }

    /// The callee name a call counts under: the identifier itself, or the
    /// member property (`React.useState(...)` spells the same token).
    fn callee_name<'a>(expr: &'a Expression<'a>) -> Option<&'a str> {
        match expr {
            Expression::Identifier(ident) => Some(ident.name.as_str()),
            Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
            _ => None,
        }
    }

    /// The `$`-rune object under a member callee: `$state.raw(...)` counts
    /// under the rune because the object identifier is the rune.
    fn rune_object<'a>(expr: &'a Expression<'a>) -> Option<&'a str> {
        match expr {
            Expression::StaticMemberExpression(member) => match &member.object {
                Expression::Identifier(ident) => Some(ident.name.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The base identifier of an `x.current = ...` assignment target.
    fn current_write_base<'a>(target: &'a AssignmentTarget<'a>) -> Option<&'a str> {
        let AssignmentTarget::StaticMemberExpression(member) = target else {
            return None;
        };
        if member.property.name.as_str() != "current" {
            return None;
        }
        match &member.object {
            Expression::Identifier(ident) => Some(ident.name.as_str()),
            _ => None,
        }
    }

    /// The reason kind a call counts under: the rune spellings rename
    /// (`$state` → `svelte-state`), everything else keeps the token.
    fn kind_of(&self, name: &str) -> Option<&'static str> {
        match name {
            "$state" => Some("svelte-state"),
            "$effect" => Some("svelte-effect"),
            other => self.calls.iter().find(|c| **c == other).copied(),
        }
    }
}

impl<'a> Visit<'a> for Scanner {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(kind) = Self::callee_name(&it.callee).and_then(|name| self.kind_of(name)) {
            self.hit(it.span, kind);
        }
        // `$state.raw(...)` / `$effect.pre(...)`: the rune is the object.
        if let Some(rune) = Self::rune_object(&it.callee) {
            match rune {
                "$state" => self.hit(it.span, "svelte-state"),
                "$effect" => self.hit(it.span, "svelte-effect"),
                _ => {}
            }
        }
        // `emit(...)` / `ctx.emit(...)` / `this.$emit(...)` (the Vue arm;
        // `defineEmits` rides the call set above).
        if self.emit_arms {
            let emits = match &it.callee {
                Expression::Identifier(ident) => matches!(ident.name.as_str(), "emit" | "$emit"),
                Expression::StaticMemberExpression(member) => {
                    matches!(member.property.name.as_str(), "emit" | "$emit")
                }
                _ => false,
            };
            if emits {
                self.hit(it.span, "emit");
            }
        }
        walk_call_expression(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &oxc_ast::ast::VariableDeclarator<'_>) {
        // `const el = useRef(null)` — record the identifier so the
        // assignment pass can catch `el.current = ...`.
        if let (
            oxc_ast::ast::BindingPattern::BindingIdentifier(id),
            Some(Expression::CallExpression(call)),
        ) = (&it.id, &it.init)
            && let Some(name) = Self::callee_name(&call.callee)
            && name == "useRef"
        {
            self.refs.insert(id.name.to_string());
        }
        walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &oxc_ast::ast::AssignmentExpression<'a>) {
        if let Some(base) = Self::current_write_base(&it.left)
            && self.refs.contains(base)
        {
            self.hit(it.span, "ref-current-write");
        }
        walk_assignment_expression(self, it);
    }

    fn visit_jsx_attribute_item(&mut self, it: &JSXAttributeItem<'a>) {
        // The spec: ANY `on*` JSX attribute. A stray `only` over-flags;
        // the direction is safe.
        if self.jsx_on
            && let JSXAttributeItem::Attribute(attr) = it
            && let JSXAttributeName::Identifier(name) = &attr.name
            && name
                .name
                .as_str()
                .get(..2)
                .is_some_and(|p| p.eq_ignore_ascii_case("on"))
        {
            self.hit(attr.span, "on-attribute");
        }
        walk_jsx_attribute_item(self, it);
    }

    fn visit_labeled_statement(&mut self, it: &oxc_ast::ast::LabeledStatement<'a>) {
        if self.labeled_reactive && it.label.name.as_str() == "$" {
            self.hit(it.span, "svelte-reactive-statement");
        }
        walk_labeled_statement(self, it);
    }
}

/// Parse `source` under each candidate source type and collect the hits.
/// `false` when no candidate parses (the caller decides the conservative
/// arm). The first diagnostic-free parse wins; a `.ts` file that only
/// parses as TS (or the reverse) still parses under one of the two, and
/// an ambiguity that fails both lands in the conservative arm — never a
/// silent miss.
fn scan_parsed(source: &str, candidates: &[oxc_span::SourceType], scanner: &mut Scanner) -> bool {
    let allocator = oxc_allocator::Allocator::default();
    for source_type in candidates {
        let parsed = oxc_parser::Parser::new(&allocator, source, *source_type).parse();
        if parsed.diagnostics.is_empty() {
            scanner.visit_program(&parsed.program);
            return true;
        }
    }
    false
}

/// Enrich one raw hit with its 1-based line and trimmed source line (the
/// F16 `Hit::reason` arithmetic, against the WHOLE file).
fn enrich(source: &str, hit: &RawHit) -> InteractivityReason {
    let start = hit.span.0;
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
    InteractivityReason {
        kind: hit.kind.to_string(),
        line,
        snippet: source
            .get(line_start..line_end)
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

/// The conservative parse-failure classification: the content is unknown,
/// so it hydrates (a false negative is the kill criterion; an extra
/// island only costs bytes).
fn unparsed() -> Interactivity {
    Interactivity {
        interactive: true,
        reasons: vec![InteractivityReason {
            kind: "unparsed".to_string(),
            line: 1,
            snippet: String::new(),
        }],
    }
}

/// Byte range of the inner body of every `<tag ...>...</tag>` block (each
/// tag open to its next close, in document order — a Svelte module
/// script block and the instance script both count). Attribute values
/// containing `>` would confuse the open-tag scan; the miss direction is
/// the conservative one (a script block not found is a module without
/// script arms, never a silent event miss).
fn block_bodies(source: &str, tag: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut cursor = 0usize;
    while let Some(rel) = source[cursor..].find(&open) {
        let open_at = cursor + rel;
        let after = open_at + open.len();
        if !matches!(
            source[after..].chars().next(),
            Some(' ' | '\t' | '\n' | '\r' | '>')
        ) {
            cursor = after;
            continue;
        }
        let Some(open_end_rel) = source[after..].find('>') else {
            break;
        };
        let body_start = after + open_end_rel + 1;
        let Some(close_rel) = source[body_start..].find(&close) else {
            break;
        };
        let body_end = body_start + close_rel;
        out.push((body_start, body_end));
        cursor = body_end + close.len();
    }
    out
}

/// The first `<template ...>` open to the LAST `</template>` close — the
/// superset the Vue template arms scan (nested `<template #slot>` tags
/// ride along; `@media` in a stray inline style over-flags, it cannot
/// under-flag). `None` when the SFC has no template block (a
/// render-function component: script arms only).
fn template_region(source: &str) -> Option<(usize, usize)> {
    let open = "<template";
    let open_rel = source.find(open)?;
    let after = open_rel + open.len();
    match source[after..].chars().next() {
        Some(' ' | '\t' | '\n' | '\r' | '>') => {}
        _ => return None,
    }
    let close = "</template>";
    let close_rel = source.rfind(close)?;
    if close_rel < after {
        return None;
    }
    Some((after, close_rel))
}

/// Scan the template region for the Vue template arms: `v-on:` and the
/// `@event` shorthand. Byte offsets stay whole-file (the region is a
/// slice of the source), so line enrichment is direct.
fn scan_vue_template(source: &str, region: (usize, usize), hits: &mut Vec<RawHit>) {
    let (start, end) = region;
    let text = source.get(start..end).unwrap_or("");
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find("v-on:") {
        let at = cursor + rel;
        hits.push(RawHit {
            span: (start + at, start + at + 5),
            kind: "vue-template-v-on",
        });
        cursor = at + 5;
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for window in chars.windows(2) {
        let (at, c) = window[0];
        if c == '@' && window[1].1.is_alphabetic() {
            hits.push(RawHit {
                span: (start + at, start + at + 1),
                kind: "vue-template-at",
            });
        }
    }
}

/// Scan the markup for the Svelte template arms: the `on:` directive, the
/// `bind:` directive and the Svelte 5 event-property spelling
/// (`onclick={...}`). The script bodies are blanked out first (byte for
/// byte, so offsets stay whole-file) so a string inside the script never
/// fires a template arm.
fn scan_svelte_markup(source: &str, scripts: &[(usize, usize)], hits: &mut Vec<RawHit>) {
    let mut buf: Vec<u8> = source.as_bytes().to_vec();
    for (start, end) in scripts {
        // Both ends are char boundaries (they come from `str::find`), so
        // blanking the byte range keeps the UTF-8 valid.
        if let Some(slot) = buf.get_mut(*start..*end) {
            slot.fill(b' ');
        }
    }
    let text = std::str::from_utf8(&buf).unwrap_or_default();

    for (needle, kind) in [("on:", "svelte-on-directive"), ("bind:", "svelte-bind")] {
        let mut cursor = 0usize;
        while let Some(rel) = text[cursor..].find(needle) {
            let at = cursor + rel;
            hits.push(RawHit {
                span: (at, at + needle.len()),
                kind,
            });
            cursor = at + needle.len();
        }
    }
    // The Svelte 5 event property: `on` + lowercase letters + `=`, all
    // byte-level (multi-byte UTF-8 bytes are never ASCII letters, so the
    // walk cannot split a character).
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        if bytes[i] == b'o' && bytes[i + 1] == b'n' && bytes[i + 2].is_ascii_lowercase() {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_lowercase() {
                j += 1;
            }
            if j > i + 2 && bytes.get(j) == Some(&b'=') {
                hits.push(RawHit {
                    span: (i, j + 1),
                    kind: "svelte-template-event",
                });
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// §V2-F02: the per-file scan — `scan_interactivity(source, framework)`
/// verbatim from the spec's design block.
#[must_use]
pub fn scan_interactivity(source: &str, framework: Framework) -> Interactivity {
    let _span =
        tracing::debug_span!("scan_interactivity", framework = framework.as_str()).entered();
    let mut hits: Vec<RawHit> = Vec::new();
    match framework {
        Framework::React => {
            let mut scanner = Scanner::react();
            let parsed = scan_parsed(
                source,
                &[oxc_span::SourceType::tsx(), oxc_span::SourceType::ts()],
                &mut scanner,
            );
            hits.append(&mut scanner.hits);
            if !parsed {
                return unparsed();
            }
        }
        Framework::Solid => {
            let mut scanner = Scanner::solid();
            let parsed = scan_parsed(
                source,
                &[oxc_span::SourceType::tsx(), oxc_span::SourceType::ts()],
                &mut scanner,
            );
            hits.append(&mut scanner.hits);
            if !parsed {
                return unparsed();
            }
        }
        Framework::Vue => {
            let scripts = block_bodies(source, "script");
            let mut parsed_all = true;
            for (start, end) in &scripts {
                let body = source.get(*start..*end).unwrap_or("");
                let mut scanner = Scanner::vue_script();
                let parsed = scan_parsed(
                    body,
                    &[oxc_span::SourceType::ts(), oxc_span::SourceType::tsx()],
                    &mut scanner,
                );
                // The scanner's spans are body-relative; shift them to
                // whole-file so the lines are.
                hits.extend(scanner.hits.drain(..).map(|mut hit| {
                    hit.span.0 += start;
                    hit.span.1 += start;
                    hit
                }));
                parsed_all &= parsed;
            }
            if let Some(region) = template_region(source) {
                scan_vue_template(source, region, &mut hits);
            }
            // A script block that does not parse could hide anything.
            if !parsed_all {
                return unparsed();
            }
        }
        Framework::Svelte => {
            let scripts = block_bodies(source, "script");
            let mut parsed_all = true;
            for (start, end) in &scripts {
                let body = source.get(*start..*end).unwrap_or("");
                let mut scanner = Scanner::svelte_script();
                let parsed = scan_parsed(
                    body,
                    &[oxc_span::SourceType::ts(), oxc_span::SourceType::tsx()],
                    &mut scanner,
                );
                hits.extend(scanner.hits.drain(..).map(|mut hit| {
                    hit.span.0 += start;
                    hit.span.1 += start;
                    hit
                }));
                parsed_all &= parsed;
            }
            scan_svelte_markup(source, &scripts, &mut hits);
            if !parsed_all {
                return unparsed();
            }
        }
        // A non-component module has no direct arms; the fixpoint still
        // promotes it when it imports an interactive component.
        Framework::None => return Interactivity::pure(),
    }
    let reasons = hits.iter().map(|hit| enrich(source, hit)).collect();
    Interactivity {
        interactive: !hits.is_empty(),
        reasons,
    }
}

/// The §V2-F02 project manifest: `{ file: { interactive, reasons } }` —
/// the per-file scan plus the import fixpoint (a component is pure when
/// it has no hits AND imports no interactive component from the same
/// project). Serialized transparently, so the JSON object is the map
/// itself, file paths as keys, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IslandManifest {
    files: BTreeMap<String, Interactivity>,
}

impl IslandManifest {
    /// Scan every file and run the fixpoint. Keys are the project-root
    /// relative paths the side-car names; sources must be the file
    /// contents (the module never touches the filesystem).
    #[must_use]
    pub fn build(files: &BTreeMap<String, (Framework, String)>) -> Self {
        let mut entries: BTreeMap<String, Interactivity> = files
            .iter()
            .map(|(path, (fw, src))| (path.clone(), scan_interactivity(src, *fw)))
            .collect();

        // The import graph: relative specifiers only, resolved against
        // the scanned set (a specifier that does not name a scanned file
        // is a package or asset import: no same-project evidence).
        let mut importers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut import_targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (path, (_fw, src)) in files {
            for sp in scan_import_spans(src) {
                if sp.specifier.contains('?') {
                    continue;
                }
                if let Some(target) = resolve_relative(path, &sp.specifier, files) {
                    importers
                        .entry(target.to_string())
                        .or_default()
                        .push(path.clone());
                    import_targets
                        .entry(path.clone())
                        .or_default()
                        .push(target.to_string());
                }
            }
        }

        // BFS over the reverse edges from the seeded interactive set: an
        // importer of an interactive file is interactive, transitively.
        let seeds: Vec<String> = entries
            .iter()
            .filter(|(_, inter)| inter.interactive)
            .map(|(path, _)| path.clone())
            .collect();
        let mut seen: BTreeSet<String> = seeds.iter().cloned().collect();
        let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
        while let Some(target) = queue.pop_front() {
            for importer in importers.get(&target).into_iter().flatten() {
                if seen.insert(importer.clone()) {
                    queue.push_back(importer.clone());
                }
            }
        }

        // Reasons for the promoted files: the interactive same-project
        // imports that pulled them in. A file with its own hits keeps
        // those (the promotion is already visible in `interactive`).
        let interactive_now: BTreeSet<String> = seen;
        let promoted: Vec<String> = interactive_now
            .iter()
            .filter(|path| {
                entries
                    .get(*path)
                    .is_some_and(|inter| inter.reasons.is_empty())
            })
            .cloned()
            .collect();
        for path in promoted {
            let reasons: Vec<InteractivityReason> = import_targets
                .get(&path)
                .into_iter()
                .flatten()
                .filter(|target| interactive_now.contains(*target))
                .map(|target| InteractivityReason {
                    kind: "imports-interactive".to_string(),
                    line: 1,
                    snippet: target.clone(),
                })
                .collect();
            if let Some(entry) = entries.get_mut(&path) {
                entry.reasons = reasons;
                entry.interactive = true;
            }
        }

        Self { files: entries }
    }

    /// The classification for one file, when the manifest scanned it.
    #[must_use]
    pub fn get(&self, file: &str) -> Option<&Interactivity> {
        self.files.get(file)
    }

    /// Every scanned file with its classification, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Interactivity)> {
        self.files.iter()
    }

    /// The files the fixpoint classified interactive, sorted — the auto
    /// hydration set the CLI registry merge consumes.
    #[must_use]
    pub fn interactive_files(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|(_, inter)| inter.interactive)
            .map(|(path, _)| path.as_str())
            .collect()
    }

    /// The side-car JSON: the spec's `{ file: { interactive, reasons } }`
    /// map, compact and sorted (the manifest-write convention).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Read a side-car back. A malformed payload is a typed
    /// `AST-ISL-001` refusal — the file the build wrote is the contract
    /// the hydration merge consumes, and a corrupt one must be named,
    /// not silently ignored.
    ///
    /// # Errors
    ///
    /// When `json` is not the expected `{ file: { interactive, reasons } }`
    /// map (`AST-ISL-001`).
    pub fn from_json(json: &str) -> Result<Self, crate::OxideError> {
        serde_json::from_str::<IslandManifest>(json).map_err(|e| {
            crate::err(
                "AST-ISL-001",
                format!(
                    "the island manifest side-car is not the expected `{{ file: {{ interactive, reasons }} }}` map: {e}"
                ),
            )
        })
    }
}

/// §V2-F02: should this island candidate hydrate? An explicit registry
/// entry always wins (the spec: "explicit `islands` registration always
/// wins"); a pure classification is the only skip; missing manifest data
/// hydrates (no classification is no vouch, and a skipped island that
/// should have hydrated is the silent breakage the kill criterion
/// forbids).
#[must_use]
pub fn hydrates(explicit_registration: bool, entry: Option<&Interactivity>) -> bool {
    explicit_registration || entry.is_none_or(Interactivity::is_interactive)
}

/// Resolve one relative specifier against `from_file`'s directory and
/// the scanned set. Returns the matching manifest KEY (the canonical
/// root-relative path) when the specifier names a same-project file.
fn resolve_relative<'a>(
    from_file: &str,
    spec: &str,
    files: &'a BTreeMap<String, (Framework, String)>,
) -> Option<&'a str> {
    if !spec.starts_with('.') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    let base = std::path::Path::new(from_file).parent()?;
    for component in base.components() {
        parts.push(component.as_os_str().to_str()?);
    }
    for component in spec.split('/') {
        match component {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    let candidates = [
        joined.clone(),
        format!("{joined}.tsx"),
        format!("{joined}.ts"),
        format!("{joined}.jsx"),
        format!("{joined}.js"),
        format!("{joined}.mts"),
        format!("{joined}.vue"),
        format!("{joined}.svelte"),
        format!("{joined}/index.tsx"),
        format!("{joined}/index.ts"),
        format!("{joined}/index.jsx"),
        format!("{joined}/index.js"),
    ];
    candidates
        .iter()
        .find_map(|candidate| files.get_key_value(candidate).map(|(key, _)| key.as_str()))
}
