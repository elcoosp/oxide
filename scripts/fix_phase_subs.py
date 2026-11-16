#!/usr/bin/env python3
"""M0 residual fix: re-record PHASE_SUBS 472 so the phase reconstruction
matches the committed phase_plan vendor_framework block (the island-twins
session evolved plan.rs after recording the sub and was cut off before
re-recording). Additive gate-script edit per the recorded-subs convention.
"""
import re

REPO = '/home/z/my-project'
PLAN = REPO + '/crates/cli/src/build/plan.rs'
SELFCHECK = REPO + '/scripts/split_selfcheck.py'

# Locate the F01 block dynamically (rustfmt may re-flow and shift lines):
# from the `// §V2-F01:` comment through the if-block's closing brace.
lines = open(PLAN).read().split('\n')
start = next(i for i, t in enumerate(lines) if t.strip().startswith('// §V2-F01:'))
end = next(i for i in range(start, len(lines))
           if lines[i].rstrip() == '    }' and lines[i + 1].strip().startswith('// §15-E:'))
block = '\n'.join(lines[start:end + 1])
assert block.startswith('    // §V2-F01:'), block[:80]
assert block.rstrip().endswith('}'), block[-80:]
assert 'let vendor_framework' in block and 'plan_island_boot(&vendor_framework' in block

anchor = '    // §15-E: JS-plugin resolveId / load hooks. Build once; both envs share'
now = block + '\n' + anchor

src = open(SELFCHECK).read()
pat = re.compile(r"    472: \(\n        '// §15-E[^\n]*',\n        '.*?',\n    \),", re.S)
m = pat.search(src)
assert m, 'sub 472 tuple not found'
# python single-quoted literal with \n escapes; content has no single quotes
literal = now.replace('\\', '\\\\').replace("'", "\\'").replace('\n', '\\n')
replacement = "    472: (\n        '// §15-E: JS-plugin resolveId / load hooks. Build once; both envs share',\n        '%s',\n    )," % literal
src = src[:m.start()] + replacement + src[m.end():]
open(SELFCHECK, 'w').write(src)
print('PHASE_SUBS 472 re-recorded: %d chars of now-payload' % len(literal))

# --- the islands.json write sub (M1): anchor `std::fs::write(&boot_path, boot_src)?;`
import subprocess, sys
sys.path.insert(0, REPO + '/scripts')
import importlib
import split_selfcheck as sc
importlib.reload(sc)
from build_phase_split import SPANS
from split_selfcheck import norm

ANCHOR = 'std::fs::write(&boot_path, boot_src)?;'
head = subprocess.run(['git', '-C', REPO, 'show', 'aa0159ac:crates/cli/src/build/cmd.rs'],
                      capture_output=True, text=True, check=True).stdout
hlines = head.split('\n')
lo, hi = SPANS['plan']
idx = None
for i in range(lo, hi + 1):
    if norm(hlines[i - 1]) == norm(ANCHOR):
        idx = i
        break
assert idx, 'anchor line not found in the aa0159ac plan span'

# the live block: the anchor line plus the added write_islands_json call
plines = open(PLAN).read().split('\n')
astart = next(i for i, t in enumerate(plines) if t.strip() == ANCHOR)
added = []
for t in plines[astart + 1:]:
    if not t.strip() or t.strip().startswith('//'):
        break
    added.append(t)
    if t.rstrip().endswith('?;') or t.rstrip().endswith(';'):
        break
assert any('write_islands_json' in t for t in added), added
was = hlines[idx - 1].rstrip('\n')
now = was + '\n' + '\n'.join(added)

src = open(SELFCHECK).read()
entry_pat = re.compile(r"    %d: \(\n        [^\n]*',\n        '.*?',\n    \)," % idx, re.S)
literal2 = now.replace('\\', '\\\\').replace("'", "\\'").replace('\n', '\\n')
entry = "    %d: (\n        '%s',\n        '%s',\n    )," % (idx, was.replace("'", "\\'"), literal2)
if entry_pat.search(src):
    src = entry_pat.sub(entry.replace('\\', '\\\\'), src, count=1)
    print('PHASE_SUBS %d refreshed' % idx)
else:
    # insert after the 472 entry (the dict literal ends with `),\n}`)
    marker = re.search(r"    472: \(\n.*?\n    \),\n", src, re.S)
    assert marker, 'no 472 entry to anchor after'
    src = src[:marker.end()] + entry + '\n' + src[marker.end():]
    print('PHASE_SUBS %d added (anchor %r, +%d lines)' % (idx, ANCHOR, len(added)))
open(SELFCHECK, 'w').write(src)
