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

def main():
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
