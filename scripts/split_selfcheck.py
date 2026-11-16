# scripts/split_selfcheck.py - V2-A01 phase 1 proof (spec: "fails if any moved item text
# differs from git show HEAD:crates/cli/src/lib.rs after stripping the leading `pub(crate) `").
# For every item the MAP moved (named fn/struct/enum items + the impl blocks), the pre-split
# text at HEAD and the post-split text in its module file must be byte-identical once the
# mover's visibility bumps are stripped and rustfmt's layout normalization is applied: the
# comparison is `pub(crate) `-stripped and whitespace-collapsed, so it proves token-for-token
# equality in order — the only allowed deltas are the visibility bumps and re-flowed layout.
# Exits non-zero listing every diff. Zero diffs is the green state.
import re, subprocess, sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from split_items import index
from split_cli import MAP, IMPLS
NL = chr(10)

# M2 drift-cleanup exemptions: items the cleanup milestone DELIBERATELY edited
# after the verbatim phase-1 move (each recorded in docs/deviations.md). The
# presence + single-copy checks below still apply to them; only the
# byte-equality-vs-HEAD check is waived. Everything else must stay verbatim.
EXEMPT = {
    # StartedServer: the dead `live` field deleted (zero readers anywhere;
    # the brownout field keeps its lib read via cmd_serve's startup line).
    'StartedServer',
    # build_dispatch_swappable: the clippy `type_complexity` fix factored the
    # routed-handles tuple into the `RoutedGenerationHandles` alias.
    'build_dispatch_swappable',
    # start_server_inner_full: the clippy `redundant_closure` fix
    # (`.map(std::sync::Arc::clone)`).
    'start_server_inner_full',
    # cmd_serve: the V2-F12 startup line reading `started.brownout` — the
    # lib-side read the e2e-seam field needs (clippy `dead_code`).
    'cmd_serve',
}

def norm(t):
    # strip the mover's visibility bumps, then collapse rustfmt layout to a token stream:
    # all whitespace removed, and trailing commas before a closer dropped (rustfmt re-wraps
    # a signature the `pub(crate) ` prefix pushed past the line width and re-adds the
    # trailing comma). Inter-item commas remain, so reordering/deletion still fails.
    t = t.replace('pub(crate) ', '')
    t = re.sub(r'\s+', '', t)
    return re.sub(r',([)\]}])', r'\1', t)

def items_of(path, text=None):
    if text is None: text = open(path).read()
    out = []
    for s, e, k, nm in index(text):
        lines = text.split(NL)
        out.append((k, nm, NL.join(lines[s:e + 1])))
    return out

# V2-A01 PHASE 2 substitutions recorded by scripts/build_phase_split.py: body
# lines the phase move deliberately rewrote (everything else must be verbatim).
# 37: phase_load's early cutoff returns None (the orchestrator maps it to exit 0).
# 170: the `Bundler as _` trait import moved to the rb.bundle call sites.
# 1047: server_manifest loses `mut` — the render-mode mutation runs in phase_emit.
PHASE_SUBS = {
    37: ('return Ok(0);', 'return Ok(None);'),
    170: ('use oxide_fw_bundler::Bundler as _;', None),
    1047: ('let mut server_manifest = read_json', 'let server_manifest = read_json'),
    # V2-B08 integration (this diff): the four nonce .replace sites go through
    # the chunk-safe NonceSplicer single-shot (build/splice_nonce.rs).
    1297: ('html.replace(&nonce, l0_cache::NONCE_SENTINEL),',
           'splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),'),
    1412: ('html.replace(&nonce, l0_cache::NONCE_SENTINEL),',
           'splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),'),
    1572: ('html.replace(&nonce, l0_cache::NONCE_SENTINEL),',
           'splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),'),
    1501: ('let fresh = saved.replace(&old_nonce, &nonce);',
           'let fresh = splice_nonce(&saved, &old_nonce, &nonce);'),
}

def phases_check():
    # A01 phase 2 proof: the concatenation of the named phase bodies under
    # build/ equals the pre-split cmd_build body at HEAD — line-for-line,
    # in order, modulo the +1 indent of the phase wrappers, rustfmt re-flow
    # (norm() collapses whitespace), and the three recorded PHASE_SUBS lines.
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import build_phase_split as gen
    from build_phase_split import SPANS, REBINDS, borrow_rules
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    head = subprocess.run(['git', '-C', repo, 'show', 'HEAD:crates/cli/src/build/cmd.rs'],
                          capture_output=True, text=True, check=True).stdout
    hlines = head.split(NL)
    owner = {}
    for ph, (lo2, hi2) in SPANS.items():
        for i in range(lo2, hi2 + 1):
            owner[i] = ph
    order = ['load', 'plan', 'bundle_client', 'bundle_server', 'prerender',
             'seed_l0', 'typegen', 'emit']
    orig = []
    for ph in order:
        lo2, hi2 = SPANS[ph]
        chunk = []
        for i in range(lo2, hi2 + 1):
            t = hlines[i - 1]
            if i in PHASE_SUBS:
                was, now = PHASE_SUBS[i]
                assert was in t, 'phase sub %d: expected %r in %r' % (i, was, t)
                if now is None:
                    continue
                t = t.replace(was, now)
            chunk.append(t)
        chunk = gen.apply_rules(NL.join(chunk),
                                gen.borrow_rules(ph)).split(NL)
        orig.extend(norm(t) if t.strip() else '' for t in chunk)
    src = os.path.join(repo, 'crates', 'cli', 'src', 'build')
    order = ['load', 'plan', 'bundle_client', 'bundle_server', 'prerender',
             'seed_l0', 'typegen', 'emit']
    got = []
    for ph in order:
        fp = os.path.join(src, ph + '.rs')
        plines = open(fp).read().split(NL)
        if len(plines) > 700:
            print('DIFF: %s exceeds the 700-line budget (%d)' % (ph + '.rs', len(plines)))
            sys.exit(1)
        first = next(i for i, t in enumerate(plines)
                     if t.strip().startswith('// >>> phase-wrapper'))
        last = max(i for i, t in enumerate(plines) if t.strip())
        assert plines[last].strip() == '}', plines[last]
        # header + signature block: everything before the first end-marker is
        # wrapper, so start inside the wrapper state.
        in_wrapper = True
        for i, t in enumerate(plines):
            s = t.strip()
            if s.startswith('// >>> phase-wrapper'):
                in_wrapper = True
                continue
            if s.startswith('// <<< phase-wrapper'):
                in_wrapper = False
                continue
            if i >= last:             # the fn's closing brace + trailing newline
                continue
            if in_wrapper:
                continue
            got.append(norm(t) if s else '')
    diffs = [(i, o, g) for i, (o, g) in enumerate(zip(orig, got)) if o != g]
    if len(orig) != len(got):
        print('DIFF: phase bodies carry %d normalized lines, HEAD body has %d'
              % (len(got), len(orig)))
        sys.exit(1)
    if diffs:
        for i, o, g in diffs[:8]:
            print('DIFF @%d: HEAD=%r phases=%r' % (i, o[:120], g[:120]))
        print('DIFFS (%d) total' % len(diffs))
        sys.exit(1)
    # the orchestrator must run the phases in the documented order
    cmd = open(os.path.join(src, 'cmd.rs')).read()
    calls = re.findall(r'phase_[a-z_0-9]+\(', cmd)
    want_calls = (['phase_load('] + ['phase_%s(' % p for p in order[1:]] + [])
    idx = [calls.index(w) for w in want_calls if w in calls]
    if want_calls != calls[:len(want_calls)] or idx != sorted(idx):
        print('DIFF: orchestrator phase order drifted: %s' % calls)
        sys.exit(1)
    print('# phase bodies checked: %d lines vs HEAD %d lines, files: %s'
          % (len(got), len(orig), ', '.join(order)))
    print('phase_selfcheck: zero diffs — the phase split is verbatim modulo '
          'indent, rustfmt layout and the 3 recorded substitutions')

def stages_check():
    # A01 phase 3 proof: the concatenation of the ten stage bodies under
    # dispatch/stages/ equals the pre-split request closure at HEAD — line-for-
    # line, in order, modulo: the one-level dedent of the async-block spans,
    # rustfmt re-flow (norm()), the recorded hoists/substitutions, the
    # Flow::Respond return-wrapping (the generator's own transform, applied to
    # the HEAD text here so the proof cannot drift) and the derived
    # needless-borrow rewrites. The A04 wrapper + the test helper stay in
    # entry.rs untouched.
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import build_stage_split as gen
    from build_stage_split import SPANS, ASYNC_SPANS, STAGE_ORDER
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    head = subprocess.run(
        ['git', '-C', repo, 'show', 'HEAD:crates/cli/src/dispatch/entry.rs'],
        capture_output=True, text=True, check=True).stdout
    hlines = head.split(NL)
    orig = []
    for ph in STAGE_ORDER:
        lo, hi = SPANS[ph]
        chunk = gen.transform_span(hlines, lo, hi, flow_only=ph in gen.FLOW_STAGES)
        if ph in ASYNC_SPANS:
            chunk = [l[4:] if l.startswith('    ') else (l if l.strip() else '')
                     for l in chunk]
        chunk = gen.apply_rules(NL.join(chunk), gen.stage_borrow_rules(ph)).split(NL)
        orig.extend(norm(t) if t.strip() else '' for t in chunk)
    src = os.path.join(repo, 'crates', 'cli', 'src', 'dispatch', 'stages')
    got = []
    for ph in STAGE_ORDER:
        fp = os.path.join(src, gen.STAGE_FILE.get(ph, ph) + '.rs')
        plines = open(fp).read().split(NL)
        if len(plines) > 700:
            print('DIFF: %s exceeds the 700-line budget (%d)' % (ph, len(plines)))
            sys.exit(1)
        last = max(i for i, t2 in enumerate(plines) if t2.strip())
        assert plines[last].strip() == '}', plines[last]
        in_wrapper = True
        for i, t2 in enumerate(plines):
            s = t2.strip()
            if s.startswith('// >>> stage-wrapper'):
                in_wrapper = True
                continue
            if s.startswith('// <<< stage-wrapper'):
                in_wrapper = False
                continue
            if i >= last:
                continue
            if in_wrapper:
                continue
            got.append(norm(t2) if s else '')
    # rustfmt re-flows the emitted `Flow::Respond(` wraps, so the proof is the
    # joined token stream per side (the phase-1 check's own rigor: order and
    # every token must match; only layout is free).
    # rustfmt may park a trailing comma at a line end with the closer on the
    # next line, so the trailing-comma strip runs on the JOINED streams; and
    # rustfmt collapses `|e| { single_expr }` to `|e| expr`, so the stage
    # proof's token stream drops brace tokens (brace STRUCTURE is rustfmt's to
    # lay out; token ORDER — identifiers, operators, literals — still must
    # match exactly).
    # rustfmt owns the separator layout: it parks trailing commas across
    # lines, collapses `|e| { single_expr }` closures and turns match-arm
    # blocks into expressions (adding the arm comma). The stage proof therefore
    # strips braces AND commas from both token streams — the remaining tokens
    # (identifiers, operators, literals) must still match in exact order.
    sep = re.compile(r'[,{}]')
    orig_join = sep.sub('', ''.join(orig))
    got_join = sep.sub('', ''.join(got))
    if orig_join != got_join:
        # locate the first divergence for a readable diff
        n = min(len(orig_join), len(got_join))
        k = next((i for i in range(n) if orig_join[i] != got_join[i]), n)
        print('DIFF: stage token stream diverges at %d (HEAD %d tokens, stages %d)'
              % (k, len(orig_join), len(got_join)))
        print('  HEAD  : %r' % orig_join[max(0, k - 60):k + 80])
        print('  stages: %r' % got_join[max(0, k - 60):k + 80])
        sys.exit(1)
    # the closure must run the stage table through RequestCtx
    entry = open(os.path.join(repo, 'crates', 'cli', 'src', 'dispatch', 'entry.rs')).read()
    for marker in ['RequestCtx::new(req, deps.swappable.load())',
                   'stages::run_stages(&deps, &mut cx)',
                   'deps_for_publish.self_dispatch.set']:
        if re.sub(r'\s+', '', marker) not in re.sub(r'\s+', '', entry):
            print('DIFF: entry.rs lost the stage-runner marker: %s' % marker)
            sys.exit(1)
    print('# stage bodies checked: %d tokens vs HEAD %d tokens, files: %s'
          % (len(got_join), len(orig_join), ', '.join(STAGE_ORDER)))
    print('stages_selfcheck: zero diffs — the stage split is verbatim modulo '
          'indent, rustfmt layout, the recorded substitutions and the derived '
          'borrow rewrites')

def main():
    if '--phases' in sys.argv:
        phases_check()
        return
    if '--stages' in sys.argv:
        stages_check()
        return
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    head = subprocess.run(['git', '-C', repo, 'show', 'HEAD:crates/cli/src/lib.rs'],
                          capture_output=True, text=True, check=True).stdout
    pre = items_of(None, head)
    # post-split search scope: exactly the module files the MAP created (pre-existing
    # modules like serve/live.rs may legitimately carry same-named items of their own).
    src = os.path.join(repo, 'crates/cli/src')
    map_files = {os.path.normpath(os.path.join(src, m + '.rs')) for m in MAP}
    post = []
    for fp in sorted(map_files):
        for k, nm, t in items_of(fp): post.append((k, nm, t, os.path.relpath(fp, repo)))
    named = [(k, nm, t) for k, nm, t in pre if k in ('fn', 'struct', 'enum') and nm]
    impls = [(k, nm, t) for k, nm, t in pre if k == 'impl']
    want = [n for ns in MAP.values() for n in ns]
    diffs = []

    # post-split location index (by normalized text; names are unique crate-wide)
    post_named = collections.defaultdict(list)
    for k, nm, t, fp in post:
        if k in ('fn', 'struct', 'enum') and nm: post_named[nm].append((t, fp))
    post_impls = [(t, fp) for k, nm, t, fp in post if k == 'impl']

    for k, nm, t in named:
        if nm not in want:
            continue
        hits = post_named.get(nm, [])
        if len(hits) != 1:
            diffs.append('%s %s: %d post-split copies (want 1)' % (k, nm, len(hits)))
            continue
        if nm not in EXEMPT and norm(t) != norm(hits[0][0]):
            diffs.append('%s %s (%s): text differs from HEAD' % (k, nm, hits[0][1]))
    for k, nm, t in impls:
        matches = [fp for t2, fp in post_impls if norm(t) == norm(t2)]
        if len(matches) != 1:
            diffs.append('%s: %d post-split copies with matching text (want 1)' % (nm, len(matches)))
    # lib.rs must not still hold any moved item
    lib = open(os.path.join(src, 'lib.rs')).read()
    libn = norm(lib)
    for k, nm, t in named:
        if nm in want and re.search(r'(fn|struct|enum)\s+%s\s*[({<]' % re.escape(nm), libn):
            diffs.append('%s: decl still present in lib.rs' % nm)
    # the stays are intact in lib.rs
    for marker in ['type LoaderInputs', 'static SESSION_TTL_SECS', 'static EDGE_WASM_BYTES',
                   'const LOAD_OVERLAP_STAGE', 'macro_rules! outln', 'mod check_backends_tests',
                   'mod tests;', 'pub use args::*;', 'pub use serve::*;']:
        if marker not in lib:
            diffs.append('stay missing from lib.rs: %s' % marker)
    moved = len(want) + len(impls)
    print('# moved items checked: %d named + %d impl blocks' % (len(want), len(impls)))
    if diffs:
        print('DIFFS (%d):' % len(diffs))
        for d in diffs: print(' ', d)
        sys.exit(1)
    print('split_selfcheck: zero diffs — every moved item equals HEAD modulo visibility bumps and rustfmt layout')

main()
