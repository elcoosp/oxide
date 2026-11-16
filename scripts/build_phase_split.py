# scripts/build_phase_split.py - V2-A01 phase 2 (spec "Step 2. Phase 2, split cmd_build
# into phases"). Splits the monolithic cmd_build in crates/cli/src/build/cmd.rs into
# BuildCtx + named phase functions, one phase per file under build/, per the spec's
# Phase-2 sketch. Behavior-preserving by construction:
#   - every body line of the original cmd_build is moved VERBATIM into exactly one
#     phase file (indented one level deeper; blank lines kept);
#   - shared locals become BuildCtx fields; a phase that CREATES a value keeps the
#     original `let` binding and hands it to the context with one assignment line at
#     the phase end; a phase that CONSUMES a value re-binds it at entry through an
#     Option accessor (`as_ref().expect(...)` / `take().expect(...)`) so the body
#     text is untouched;
#   - the one deliberate body edit is phase_load's early cutoff `return Ok(0);` ->
#     `return Ok(None);` (the orchestrator maps None to the exit code).
# Wrapper lines (signatures, re-binds, hand-offs) are fenced with `// >>> phase-wrapper`
# / `// <<< phase-wrapper` markers so scripts/split_selfcheck.py --phases can strip them
# and prove the concatenated phase bodies equal the pre-split HEAD body line-for-line.
# Usage: python3 scripts/build_phase_split.py [--check-only]
import sys, os, re

NL = chr(10)
SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                   'crates', 'cli', 'src', 'build')

# (first line, last line) — 1-based, inclusive, contiguous 13..2348.
SPANS = {
    'load': (13, 122),
    'plan': (123, 660),
    'bundle_client': (661, 960),
    'bundle_server': (961, 1054),
    'prerender': (1055, 1637),
    'seed_l0': (1638, 1717),
    'typegen': (1718, 1886),
    'emit': (1887, 2348),
}

# phase files that create each shared local -> BuildCtx field hand-off lines.
# `Some(x)` wraps into an Option field; a bare `x` assigns an already-Option value.
HANDOFFS = {
    'load': ['rootp', 'dist', 'cfg', 'tree', 'routes_root', 'stack'],
    'plan': [
        ('entries', 'Some'), ('client_out', 'Some'), ('server_out', 'Some'),
        ('boot_chunk_url', 'bare'), ('server_precompiled', 'Some'),
        ('build_secret_for_fp', 'Some'), ('css_transform', 'Some'),
        ('client_opts', 'Some'), ('server_opts', 'Some'), ('rb', 'Some'),
        ('transform_html_hook', 'bare'),
    ],
    'bundle_client': [('client_outcome', 'Some')],
    'bundle_server': [
        ('client_manifest', 'Some'), ('client_manifest_snapshot', 'Some'),
        ('boot_script_url', 'bare'), ('boot_script_integrity', 'bare'),
        ('manifest_integrity', 'Some'), ('head_producer', 'bare'),
        ('server_manifest', 'Some'), ('build_id', 'Some'),
    ],
    'prerender': [
        ('prerendered', 'Some'), ('seed_bodies', 'Some'),
        ('static_override', 'Some'), ('new_prerender_records', 'Some'),
    ],
    'seed_l0': [], 'typegen': [], 'emit': [],
}

# per-phase entry re-binds: the context fields the phase reads (and how).
# `ref`    -> let name = &cx.name;            (bare field)
# `optref` -> let name = cx.name.as_ref().expect("...sets name");
# `take`   -> let name = cx.name.take().expect("...sets name");   (moved-out in-phase)
# `muttake`-> let mut name = cx.name.take().expect("...sets name");
# `clone`  -> let name = cx.css-like.clone().expect(...);          (Option<T>, T: Clone)
# `mutw`   -> let w = &mut *cx.w;
REBINDS = {
    'plan': [
        ('rootp', 'ref'), ('dist', 'ref'), ('cfg', 'ref'), ('tree', 'ref'),
        ('stack', 'ref'), ('w', 'mutw'),
    ],
    'bundle_client': [
        ('rootp', 'ref'), ('cfg', 'ref'), ('w', 'mutw'),
        ('client_out', 'optref'), ('client_opts', 'optref'), ('rb', 'optref'),
        ('build_secret_for_fp', 'optref'),
    ],
    'bundle_server': [
        ('rootp', 'ref'), ('cfg', 'ref'), ('tree', 'ref'),
        ('client_out', 'optref'), ('server_out', 'optref'),
        ('server_opts', 'optref'), ('rb', 'optref'), ('client_outcome', 'optref'),
        ('boot_chunk_url', 'ref'),
    ],
    'prerender': [
        ('rootp', 'ref'), ('dist', 'ref'), ('cfg', 'ref'), ('tree', 'ref'),
        ('routes_root', 'ref'), ('stack', 'ref'), ('w', 'mutw'),
        ('boot_script_url', 'ref'), ('boot_script_integrity', 'ref'),
        ('manifest_integrity', 'optref'), ('head_producer', 'ref'),
        ('build_id', 'optref'), ('transform_html_hook', 'ref'),
    ],
    'seed_l0': [
        ('dist', 'ref'), ('cfg', 'ref'), ('w', 'mutw'), ('build_id', 'optref'),
        ('seed_bodies', 'take'), ('prerendered', 'optref'),
    ],
    'typegen': [('rootp', 'ref'), ('dist', 'ref'), ('cfg', 'ref'), ('tree', 'ref'),
                ('w', 'mutw')],
    'emit': [
        ('rootp', 'ref'), ('dist', 'ref'), ('cfg', 'ref'), ('tree', 'ref'),
        ('routes_root', 'ref'), ('w', 'mutw'),
        ('r#static', 'copy'), ('explain', 'copy'), ('attest', 'copy'), ('key_path', 'copy'),
        ('entries', 'optref'), ('client_out', 'optref'), ('server_out', 'optref'),
        ('server_precompiled', 'optref'), ('css_transform', 'clone'), ('stack', 'ref'),
        ('static_override', 'optref'),
        ('client_manifest', 'take'), ('client_manifest_snapshot', 'optref'),
        ('server_manifest', 'muttake'), ('build_id', 'optref'),
        ('prerendered', 'take'), ('new_prerender_records', 'muttake'),
    ],
    'load': [],
}

WHO_SETS = {
    'client_out': 'phase_plan', 'server_out': 'phase_plan', 'entries': 'phase_plan',
    'server_precompiled': 'phase_plan', 'build_secret_for_fp': 'phase_plan',
    'css_transform': 'phase_plan', 'client_opts': 'phase_plan', 'server_opts': 'phase_plan',
    'rb': 'phase_plan',
    'client_outcome': 'phase_bundle_client',
    'client_manifest': 'phase_bundle_server', 'client_manifest_snapshot': 'phase_bundle_server',
    'manifest_integrity': 'phase_bundle_server', 'server_manifest': 'phase_bundle_server',
    'build_id': 'phase_bundle_server',
    'seed_bodies': 'phase_prerender', 'prerendered': 'phase_prerender',
    'static_override': 'phase_prerender', 'new_prerender_records': 'phase_prerender',
}

PHASE_DOCS = {
    'load': ("phase_load — entry, early cutoff, config + route scan, framework stack\n"
             "/// (spec WP-A01 phase 2, the sketch's `phase_load`). Canonicalizes the root\n"
             "/// once, short-circuits an up-to-date build (returns `None` -> exit 0), loads\n"
             "/// the production config, fires the `config`/`configResolved` plugin hooks,\n"
             "/// fails on unknown native plugins, scans + refines the route tree and builds\n"
             "/// the framework stack. Returns the fully-seeded [`BuildCtx`]."),
    'plan': ("phase_plan — entry sets, boot entry, precompile pass, bundle options\n"
             "/// (spec WP-A01 phase 2, the front half of `phase_bundle_client`). Builds the\n"
             "/// client/server entry maps, the synthetic `__boot__` entry, the Tier-B-backed\n"
             "/// precompile pass for both envs, the config bundle, the transform hooks, the\n"
             "/// two `BundleOptions` (svg?react + img?w virtual modules included) and the\n"
             "/// `RolldownBundler`. Produces everything the two bundle phases read."),
    'bundle_client': ("phase_bundle_client — the client env bundle + its gates\n"
                      "/// (spec WP-A01 phase 2). Runs the incremental client bundle (§5-A env\n"
                      "/// fingerprint skip) and the post-bundle gates: the V8 `node:` requirement\n"
                      "/// scan (JSH-V8KIND), the forbidden server-export guard (CMP-GUARD) and\n"
                      "/// the client secret scan (CMP-SECRET), then records the env fingerprint."),
    'bundle_server': ("phase_bundle_server — the SSR env bundle + the manifest reads\n"
                      "/// (spec WP-A01 phase 2). Bundles the server env, enriches both manifests\n"
                      "/// with layout chains, persists diagnostics, reads client/server manifests\n"
                      "/// and derives the boot script URL/integrity, the manifest integrity map,\n"
                      "/// the E04 head producer and the canonical build id."),
    'prerender': ("phase_prerender — the §10.5 static-route prerender walk\n"
                  "/// (spec WP-A01 phase 2). Computes the nonce, walks every page route (the\n"
                  "/// literal / runtime / absent `generateParams` arms), reuses unchanged\n"
                  "/// outputs by source hash, runs `transformHtml`, archives the route tree\n"
                  "/// and collects the L0 seed bodies + prerender records."),
    'seed_l0': ("phase_seed_l0 — route search index + L0 seed manifest\n"
                "/// (spec WP-A01 phase 2). Emits the §16.5 route search index and the\n"
                "/// PERF-04 L0 seed manifest (default-locale variants only) so `oxide start`\n"
                "/// serves seeded routes as first-request hits."),
    'typegen': ("phase_typegen — routes.d.ts + i18n message catalogs\n"
                "/// (spec WP-A01 phase 2). Regenerates `.oxide/types/routes.d.ts` (locale-aware\n"
                "/// overloads under i18n) and the `messages.d.ts` + per-locale/per-route catalog\n"
                "/// emission of §16-D."),
    'emit': ("phase_emit — manifest enrichment, adapter artifacts, stamps, summary\n"
             "/// (spec WP-A01 phase 2). Infers render modes into the server manifest, fires\n"
             "/// the adapter hook, runs the §18 config refusal, compiles the edge artifacts,\n"
             "/// emits the deploy target, the typegen/sitemap/llms/PWA/fonts/snapshot tail,\n"
             "/// stamps the build id (A04) into dist + the shells, records the demand graph\n"
             "/// + fingerprint, attests, GCs the artifact cache and prints the summary."),
}


def fence(line):
    return ['    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases '
            'strips this block)', line]


def rebind_line(name, how):
    setter = WHO_SETS.get(name, 'phase_load')
    if how == 'ref':
        return f'    let {name} = &cx.{name};'
    if how == 'copy':
        return f'    let {name} = cx.{name};'
    if how == 'mutw':
        return '    let w = &mut *cx.w;'
    if how == 'optref':
        return f'    let {name} = cx.{name}.as_ref().expect("{setter} sets {name}");'
    if how == 'take':
        return f'    let {name} = cx.{name}.take().expect("{setter} sets {name}");'
    if how == 'muttake':
        return f'    let mut {name} = cx.{name}.take().expect("{setter} sets {name}");'
    if how == 'clone':
        return f'    let {name} = cx.{name}.clone().expect("{setter} sets {name}");'
    raise ValueError(how)


def borrow_rules(phase):
    # The needless_borrow fixes clippy demands once a re-bind makes a local a
    # reference: every `&name` not followed by `.` (a field access borrows the
    # FIELD — keep it) drops the redundant `&`. Derived from REBINDS so the
    # generator and split_selfcheck --phases apply the identical rule set.
    rules = []
    for name, how in REBINDS.get(phase, []):
        if how in ('ref', 'optref'):
            rules.append((re.compile(r'(?<![\w&])&%s\b(?!\s*[.\[])' % re.escape(name)), name))
        # once the borrow is dropped, `name: name` in struct literals becomes
        # the field shorthand (clippy redundant_field_names).
        if how in ('ref', 'optref'):
            rules.append((re.compile(r'(?<![\w&])(%s): %s(?=\s*[,)])' % (re.escape(name),
                                                                         re.escape(name))),
                          r'\1'))
    return rules


def apply_rules(text, rules):
    # rules run over the JOINED phase text: a `&name` at end-of-line can have
    # its `.` consumer on the next line (rustfmt-wrapped method chains), so
    # the negative lookahead must see across the newline.
    if not rules:
        return text
    for rx, repl in rules:
        text = rx.sub(repl, text)
    return text


def body_of(lines, lo, hi, mapped):
    out = []
    for i in range(lo, hi + 1):
        l = lines[i - 1]
        if l.strip() == '':
            out.append('')
        elif i in mapped:
            if mapped[i] is not None:
                out.append(mapped[i])
            # None: the line is deliberately dropped (recorded substitution)
        else:
            out.append('    ' + l)
    return out


def write_phase(fname, doc, async_, sig_args, rebinds, handoffs, body, tail_stmts,
                extra_uses=(), add_ok=True):
    w = []
    w.append('//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).')
    w.append('//!')
    for dl in doc.replace('/// ', '').split(NL):
        w.append('//! ' + dl)
    w.append('')
    w.append('use crate::*;')
    for u in extra_uses:
        w.append(u)
    w.append('')
    w.append('// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)')
    sig = f"pub(crate) {'async ' if async_ else ''}fn {sig_args}"
    w.append(sig)
    for name, how in rebinds:
        w.append(rebind_line(name, how))
    w.append('// <<< phase-wrapper')
    w.extend(body)
    if tail_stmts:
        w.append('// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)')
        w.extend(tail_stmts)
        if add_ok:
            w.append('    Ok(())')
        w.append('// <<< phase-wrapper')
    elif add_ok:
        w.append('// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)')
        w.append('    Ok(())')
        w.append('// <<< phase-wrapper')
    w.append('}')
    path = os.path.join(SRC, fname)
    with open(path, 'w') as f:
        f.write(NL.join(w) + NL)
    return len(w)


def main():
    import subprocess
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    # The pristine monolith lives at HEAD (the delivery stays dirty — NO COMMITS),
    # so the split is re-runnable: read the source of truth from git, not the worktree.
    text = subprocess.run(['git', '-C', repo, 'show', 'HEAD:crates/cli/src/build/cmd.rs'],
                          capture_output=True, text=True, check=True).stdout
    lines = text.split(NL)
    # boundary assertions — the split is only valid against this exact tree
    assert lines[11].strip() == ') -> anyhow::Result<i32> {', lines[11]
    assert lines[36].strip() == 'return Ok(0);', lines[36]
    assert lines[2348].strip() == 'Ok(0)', lines[2348]
    assert lines[2349].strip() == '}', lines[2349]
    body = lines[12:2348]  # 13..2348
    # --- ctx.rs ------------------------------------------------------------
    ctx = []
    ctx.append('//! V2-A01 phase 2: the build-phase context (spec WP-A01 "Step 2" sketch).')
    ctx.append('//!')
    ctx.append('//! `cmd_build`\'s ~2,300-line body shared its locals across the phase boundaries;')
    ctx.append('//! those locals are the [`BuildCtx`] fields here. The command arguments and the')
    ctx.append('//! canonical paths are plain fields set by [`phase_load`]; every later value is')
    ctx.append('//! an `Option` field that the phase which creates it fills in (the orchestrator')
    ctx.append('//! fixes the phase order, so a `None` read through a phase\'s `expect` is a')
    ctx.append('//! contract violation, never a run-time branch). The fields whose own value is')
    ctx.append('//! an `Option` (`boot_chunk_url`, `boot_script_url`, `boot_script_integrity`,')
    ctx.append('//! `head_producer`, `css_transform` rides `Some`, `transform_html_hook`) are')
    ctx.append('//! bare `Option<T>` so consumers never see `Option<Option<_>>`.')
    ctx.append('')
    ctx.append('use crate::*;')
    ctx.append('')
    ctx.append('/// The §6.1 entry set: route id → on-disk entry path (the alias keeps the')
    ctx.append('/// `BuildCtx` field declarations under clippy\'s type-complexity budget).')
    ctx.append('pub(crate) type EntryMap = std::collections::BTreeMap<String, std::path::PathBuf>;')
    ctx.append('')
    ctx.append('/// One L0-seed row: (route_id, url_path, sentineled html, status, locale).')
    ctx.append('pub(crate) type SeedBody = (String, String, String, u16, String);')
    ctx.append('')
    ctx.append('/// The per-build context threaded through the named phase functions.')
    ctx.append('pub(crate) struct BuildCtx<\'w> {')
    ctx.append('    // ---- core: command arguments + canonical paths (set by phase_load) ----')
    ctx.append('    /// The canonicalized project root (CONVENTIONS: path canonicalization; the')
    ctx.append("    /// sketch calls it `root` — `rootp` is the function's own local name).")
    ctx.append('    pub(crate) rootp: std::path::PathBuf,')
    ctx.append('    /// The dist/ directory (the sketch\'s `out`).')
    ctx.append('    pub(crate) dist: std::path::PathBuf,')
    ctx.append('    pub(crate) w: Writer<\'w>,')
    ctx.append('    pub(crate) r#static: bool,')
    ctx.append('    pub(crate) explain: bool,')
    ctx.append('    pub(crate) attest: bool,')
    ctx.append('    pub(crate) key_path: Option<&\'w str>,')
    ctx.append('    // ---- phase_load outputs ----')
    ctx.append('    pub(crate) cfg: oxide_fw_config::OxideConfig,')
    ctx.append('    pub(crate) tree: oxide_fw_router::RouteTree,')
    ctx.append('    pub(crate) routes_root: std::path::PathBuf,')
    ctx.append('    pub(crate) stack: crate::framework_shim::FrameworkStack,')
    ctx.append('    // ---- phase_plan outputs ----')
    ctx.append('    pub(crate) entries: Option<EntryMap>,')
    ctx.append('    pub(crate) client_out: Option<std::path::PathBuf>,')
    ctx.append('    pub(crate) server_out: Option<std::path::PathBuf>,')
    ctx.append('    pub(crate) boot_chunk_url: Option<String>,')
    ctx.append('    pub(crate) server_precompiled:')
    ctx.append('        Option<std::collections::BTreeMap<std::path::PathBuf, String>>,')
    ctx.append('    pub(crate) build_secret_for_fp: Option<String>,')
    ctx.append('    pub(crate) css_transform: Option<oxide_fw_bundler::CssTransformHook>,')
    ctx.append('    pub(crate) client_opts: Option<oxide_fw_bundler::BundleOptions>,')
    ctx.append('    pub(crate) server_opts: Option<oxide_fw_bundler::BundleOptions>,')
    ctx.append('    pub(crate) rb: Option<oxide_fw_bundler::RolldownBundler>,')
    ctx.append('    pub(crate) transform_html_hook: Option<oxide_fw_bundler::TransformHtmlHook>,')
    ctx.append('    // ---- phase_bundle_client outputs ----')
    ctx.append('    pub(crate) client_outcome: Option<oxide_fw_bundler::BundleOutput>,')
    ctx.append('    // ---- phase_bundle_server outputs ----')
    ctx.append('    pub(crate) client_manifest: Option<serde_json::Value>,')
    ctx.append('    pub(crate) client_manifest_snapshot: Option<serde_json::Value>,')
    ctx.append('    pub(crate) boot_script_url: Option<String>,')
    ctx.append('    pub(crate) boot_script_integrity: Option<String>,')
    ctx.append('    pub(crate) manifest_integrity: Option<std::collections::BTreeMap<String, String>>,')
    ctx.append('    pub(crate) head_producer: Option<oxide_fw_bundler::head::ManifestHeadProducer>,')
    ctx.append('    pub(crate) server_manifest: Option<serde_json::Value>,')
    ctx.append('    pub(crate) build_id: Option<String>,')
    ctx.append('    // ---- phase_prerender outputs ----')
    ctx.append('    pub(crate) prerendered: Option<std::collections::BTreeMap<String, Vec<String>>>,')
    ctx.append('    pub(crate) seed_bodies: Option<Vec<SeedBody>>,')
    ctx.append('    pub(crate) static_override: Option<std::collections::BTreeSet<String>>,')
    ctx.append('    pub(crate) new_prerender_records: Option<Vec<crate::build_graph::RoutePrerenderRecord>>,')
    ctx.append('}')
    with open(os.path.join(SRC, 'ctx.rs'), 'w') as f:
        f.write(NL.join(ctx) + NL)

    # --- load.rs (owns the early cutoff + struct construction) --------------
    mapped = {37: '            return Ok(None);'}
    load_body = body_of(lines, 13, 122, mapped)
    construct = [
        '    Ok(Some(BuildCtx {',
        '        rootp,',
        '        dist,',
        '        w,',
        '        r#static,',
        '        explain,',
        '        attest,',
        '        key_path,',
        '        cfg,',
        '        tree,',
        '        routes_root,',
        '        stack,',
        '        entries: None,',
        '        client_out: None,',
        '        server_out: None,',
        '        boot_chunk_url: None,',
        '        server_precompiled: None,',
        '        build_secret_for_fp: None,',
        '        css_transform: None,',
        '        client_opts: None,',
        '        server_opts: None,',
        '        rb: None,',
        '        transform_html_hook: None,',
        '        client_outcome: None,',
        '        client_manifest: None,',
        '        client_manifest_snapshot: None,',
        '        boot_script_url: None,',
        '        boot_script_integrity: None,',
        '        manifest_integrity: None,',
        '        head_producer: None,',
        '        server_manifest: None,',
        '        build_id: None,',
        '        prerendered: None,',
        '        seed_bodies: None,',
        '        static_override: None,',
        '        new_prerender_records: None,',
        '    }))',
    ]
    n = write_phase(
        'load.rs', PHASE_DOCS['load'].replace('/// ', ''), True,
        "phase_load<'w>(\n    root: &str,\n    r#static: bool,\n    explain: bool,\n"
        "    attest: bool,\n    key_path: Option<&'w str>,\n    w: Writer<'w>,\n"
        ") -> anyhow::Result<Option<BuildCtx<'w>>> {",
        [], [], load_body, construct, add_ok=False)

    # --- the seven &mut BuildCtx phases -------------------------------------
    SIGS = {
        'plan': ('phase_plan', True),
        'bundle_client': ('phase_bundle_client', True),
        'bundle_server': ('phase_bundle_server', True),
        'prerender': ('phase_prerender', True),
        'seed_l0': ('phase_seed_l0', False),
        'typegen': ('phase_typegen', False),
        'emit': ('phase_emit', True),
    }
    tails = {}
    for name, (fn, _a) in SIGS.items():
        t = []
        for h in HANDOFFS[name]:
            if isinstance(h, tuple):
                field, kind = h
                t.append(f'    cx.{field} = {"Some(" if kind == "Some" else ""}{field}'
                         f'{")" if kind == "Some" else ""};')
            else:
                t.append(f'    cx.{h} = Some({h});')
        tails[name] = t
    written = {'ctx.rs': len(ctx), 'load.rs': n}
    # The `Bundler` trait (rb.bundle) must be in scope at the two bundle call sites —
    # the trait import lived inside the old monolith body (plan keeps it verbatim).
    EXTRA = {'bundle_client': ['use oxide_fw_bundler::Bundler as _;'],
             'bundle_server': ['use oxide_fw_bundler::Bundler as _;']}
    for name, (fn, is_async) in SIGS.items():
        lo, hi = SPANS[name]
        # Mapped body substitutions (each recorded for split_selfcheck --phases):
        #  prerender lines 1297/1412/1572 + 1501: the V2-B08 integration — the
        #  nonce splice goes through the chunk-safe NonceSplicer (single-shot)
        #  instead of whole-document str::replace copies.
        #  plan line 170: the `Bundler as _` trait import moved to the two bundle
        #  phase files (the rb.bundle call sites) — dead inside phase_plan now.
        #  bundle_server line 1047: `let mut server_manifest` loses `mut` — the
        #  render-mode mutation runs in phase_emit over the moved-out value.
        mapped = {}
        if name == 'plan':
            mapped[170] = None
        if name == 'prerender':
            mapped[1297] = ('                            splice_nonce(&html, &nonce,'
                            ' l0_cache::NONCE_SENTINEL),')
            mapped[1412] = ('                                splice_nonce(&html, &nonce,'
                            ' l0_cache::NONCE_SENTINEL),')
            mapped[1572] = ('                        splice_nonce(&html, &nonce,'
                            ' l0_cache::NONCE_SENTINEL),')
            mapped[1501] = '                            let fresh = splice_nonce(&saved, &old_nonce, &nonce);'
        mapped = mapped if name in ('plan', 'prerender') else {}
        if name == 'bundle_server':
            mapped[1047] = ('    let server_manifest = '
                            'read_json(&server_out.join("server.manifest.json"))')
        rules = borrow_rules(name)
        if name == 'plan':
            mapped[170] = None
        if name == 'bundle_server':
            mapped[1047] = ('    let server_manifest = '
                            'read_json(&server_out.join("server.manifest.json"))')
        body = apply_rules(NL.join(body_of(lines, lo, hi, mapped)), rules).split(NL)
        sig = (f'{fn}(cx: &mut BuildCtx<\'_>) -> anyhow::Result<()> {{')
        written[f'{name}.rs'] = write_phase(
            f'{name}.rs', PHASE_DOCS[name].replace('/// ', ''), is_async, sig,
            REBINDS[name], HANDOFFS[name], body, tails[name],
            extra_uses=EXTRA.get(name, ()))

    # --- cmd.rs: the orchestrator -------------------------------------------
    cmd = []
    cmd.append('//! V2-A01 phase 2: `cmd_build` is now the phase orchestrator.')
    cmd.append('//!')
    cmd.append('//! The spec\'s phase sketch (`phase_load` -> `phase_bundle_client` ->')
    cmd.append('//! `phase_bundle_server` -> `phase_prerender` -> `phase_seed_l0` ->')
    cmd.append('//! `phase_emit`) maps onto the tree as: [`phase_load`], [`phase_plan`] (the')
    cmd.append('//! entry-set/options front half of the bundle phases — split out to keep every')
    cmd.append('//! phase file under the 700-line budget), [`phase_bundle_client`],')
    cmd.append('//! [`phase_bundle_server`], [`phase_prerender`], [`phase_seed_l0`],')
    cmd.append('//! [`phase_typegen`] (the §8.3/§16-D typegen tail — same budget reason) and')
    cmd.append('//! [`phase_emit`]. Bodies moved verbatim; the shared locals are the')
    cmd.append('//! `BuildCtx` fields; `scripts/split_selfcheck.py --phases` proves the')
    cmd.append('//! concatenation equals the pre-split `cmd_build` body line-for-line.')
    cmd.append('')
    cmd.append('use crate::*;')
    cmd.append('use super::*;')
    cmd.append('')
    cmd.append('/// The `oxide build` command (spec §10). Phase order is the pipeline contract:')
    cmd.append('/// every phase\'s `expect` on its inputs names the phase that must run before it.')
    cmd.append("pub(crate) async fn cmd_build(")
    cmd.append('    root: &str,')
    cmd.append('    r#static: bool,')
    cmd.append('    explain: bool,')
    cmd.append('    attest: bool,')
    cmd.append('    key_path: Option<&str>,')
    cmd.append("    w: Writer<'_>,")
    cmd.append(') -> anyhow::Result<i32> {')
    cmd.append('    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)')
    cmd.append('    let Some(mut cx) =')
    cmd.append('        phase_load(root, r#static, explain, attest, key_path, w).await?')
    cmd.append('    else {')
    cmd.append('        // §5.1 early cutoff: up to date, nothing to do (the pre-split `return Ok(0)`).')
    cmd.append('        return Ok(0);')
    cmd.append('    };')
    cmd.append('    phase_plan(&mut cx).await?;')
    cmd.append('    phase_bundle_client(&mut cx).await?;')
    cmd.append('    phase_bundle_server(&mut cx).await?;')
    cmd.append('    phase_prerender(&mut cx).await?;')
    cmd.append('    phase_seed_l0(&mut cx)?;')
    cmd.append('    phase_typegen(&mut cx)?;')
    cmd.append('    phase_emit(&mut cx).await?;')
    cmd.append('    // <<< phase-wrapper')
    cmd.append('    Ok(0)')
    cmd.append('}')
    with open(os.path.join(SRC, 'cmd.rs'), 'w') as f:
        f.write(NL.join(cmd) + NL)
    written['cmd.rs'] = len(cmd)

    # --- mod.rs ---------------------------------------------------------------
    mod = []
    mod.append('//! V2-A01 module root.')
    mod.append('')
    mod.append('mod cmd;')
    mod.append('pub(crate) use cmd::*;')
    mod.append('mod secret;')
    mod.append('pub(crate) use secret::*;')
    mod.append('mod splice_nonce;')
    mod.append('pub(crate) use splice_nonce::*;')
    mod.append('// V2-A01 phase 2: the build pipeline as named phases over BuildCtx.')
    mod.append('mod ctx;')
    mod.append('pub(crate) use ctx::*;')
    mod.append('mod load;')
    mod.append('pub(crate) use load::*;')
    for name, (fn, _a) in SIGS.items():
        mod.append(f'mod {name};')
        mod.append(f'pub(crate) use {name}::*;')
    with open(os.path.join(SRC, 'mod.rs'), 'w') as f:
        f.write(NL.join(mod) + NL)

    print('wrote:', {k: v for k, v in written.items()})
    print('total body lines moved:', len(body))


if __name__ == '__main__':
    main()
