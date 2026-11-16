#!/usr/bin/env python3
"""M2-B02: record the two prerender wrap-check substitutions (the raw-id
rule the dispatch RouteContextTable builds its chains with). Additive
PHASE_SUBS entries per the recorded-subs convention.
"""
import re
import subprocess
import sys

REPO = '/home/z/my-project'
PRERENDER = REPO + '/crates/cli/src/build/prerender.rs'
SELFCHECK = REPO + '/scripts/split_selfcheck.py'

sys.path.insert(0, REPO + '/scripts')
from build_phase_split import SPANS
from split_selfcheck import norm

ANCHORS = [
    'route_id.starts_with(&format!("{layout_base}/"))',
    'route_id.starts_with(&format!("{base}/"))',
]

plines = open(PRERENDER).read().split('\n')
head = subprocess.run(
    ['git', '-C', REPO, 'show', 'aa0159ac:crates/cli/src/build/cmd.rs'],
    capture_output=True, text=True, check=True).stdout
hlines = head.split('\n')
lo, hi = SPANS['prerender']

entries = []
for anchor in ANCHORS:
    # live lines: the code line + the comment block directly above it
    lidx = next(i for i, t in enumerate(plines) if t.strip() == anchor.replace(
        'route_id.', 'e.id.0.'))
    start = lidx
    while start > 0 and plines[start - 1].strip().startswith('//'):
        start -= 1
    live_lines = plines[start:lidx + 1]
    assert any('V2-B02' in t for t in live_lines), live_lines

    idx = next(i for i in range(lo, hi + 1) if norm(hlines[i - 1]) == norm(anchor))
    was = hlines[idx - 1].rstrip('\n')
    now = '\n'.join(live_lines)
    entries.append((idx, was, now))

src = open(SELFCHECK).read()
for idx, was, now in entries:
    literal = now.replace('\\', '\\\\').replace("'", "\\'").replace('\n', '\\n')
    was_esc = was.replace("'", "\\'")
    entry = "    %d: (\n        '%s',\n        '%s',\n    )," % (idx, was_esc, literal)
    pat = re.compile(r"    %d: \(\n        [^\n]*',\n        '.*?',\n    \)," % idx, re.S)
    if pat.search(src):
        src = pat.sub(entry.replace('\\', '\\\\'), src, count=1)
        print('PHASE_SUBS %d refreshed' % idx)
    else:
        marker = re.search(r"    218: \(\n.*?\n    \),\n", src, re.S)
        assert marker, 'no 218 entry to anchor after'
        src = src[:marker.end()] + entry + '\n' + src[marker.end():]
        print('PHASE_SUBS %d added (+%d lines: %r)' % (idx, now.count('\n') + 1 - 1, was[:40]))
open(SELFCHECK, 'w').write(src)
