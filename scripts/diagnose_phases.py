#!/usr/bin/env python3
"""Diagnose the --phases 6-line divergence per phase file."""
import os, subprocess, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_phase_split as gen
from build_phase_split import SPANS
from split_selfcheck import PHASE_SUBS, norm

repo = '/home/z/my-project'
NL = '\n'
head = subprocess.run(['git', '-C', repo, 'show', 'aa0159ac:crates/cli/src/build/cmd.rs'],
                      capture_output=True, text=True, check=True).stdout
hlines = head.split(NL)
order = ['load', 'plan', 'bundle_client', 'bundle_server', 'prerender',
         'seed_l0', 'typegen', 'emit']
src = os.path.join(repo, 'crates', 'cli', 'src', 'build')

for ph in order:
    lo, hi = SPANS[ph]
    chunk = []
    for i in range(lo, hi + 1):
        t = hlines[i - 1]
        if i in PHASE_SUBS:
            was, now = PHASE_SUBS[i]
            assert was in t, 'phase sub %d: expected %r in %r' % (i, was, t)
            if now is None:
                continue
            t = t.replace(was, now)
        chunk.append(t)
    chunk = gen.apply_rules(NL.join(chunk), gen.borrow_rules(ph)).split(NL)
    orig = [norm(t) if t.strip() else '' for t in chunk]

    fp = os.path.join(src, ph + '.rs')
    plines = open(fp).read().split(NL)
    last = max(i for i, t in enumerate(plines) if t.strip())
    in_wrapper = True
    got = []
    for i, t in enumerate(plines):
        s = t.strip()
        if s.startswith('// >>> phase-wrapper'):
            in_wrapper = True
            continue
        if s.startswith('// <<< phase-wrapper'):
            in_wrapper = False
            continue
        if i >= last:
            continue
        if in_wrapper:
            continue
        got.append(norm(t) if s else '')

    # strip trailing blanks, then align by difflib
    while orig and orig[-1] == '':
        orig.pop()
    while got and got[-1] == '':
        got.pop()
    if orig == got:
        print(f'{ph}: MATCH ({len(got)} lines)')
        continue
    import difflib
    sm = difflib.SequenceMatcher(a=orig, b=got, autojunk=False)
    print(f'{ph}: orig={len(orig)} got={len(got)}')
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            continue
        print(f'  {tag} orig[{i1}:{i2}] -> got[{j1}:{j2}]')
        for k in range(i1, min(i2, i1 + 4)):
            print(f'    - {orig[k][:110]}')
        for k in range(j1, min(j2, j1 + 4)):
            print(f'    + {got[k][:110]}')
