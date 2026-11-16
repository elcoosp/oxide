# scripts/split_items.py - lossless top-level item index for a rustfmt-formatted Rust file.
# An item starts at a column-0 line (fn/struct/enum/impl/mod/use/...) and ends at the next
# column-0 line that is exactly a closing brace; one-line items end on the same line.
# Doc comments and attributes directly above an item belong to it; an attribute group whose
# parens stay open keeps the run alive across its indented continuation lines (e.g. the
# wrapped `#[command(...)]` clap attributes), so the whole attr block moves with the item.
import re, sys
NL = chr(10)
START = re.compile(r'^(pub([(][a-z]+[)])?[ ]+)?(async[ ]+)?(unsafe[ ]+)?(fn|struct|enum|trait|impl|mod|const|static|type|use|macro_rules!)(?![A-Za-z0-9_])')
NAME = re.compile(r'(?:fn|struct|enum|trait|mod|const|static|type|macro_rules!)[ ]+([A-Za-z_][A-Za-z0-9_]*)')
DEPTH = re.compile(r'[()\[\]{}]')

def index(src):
    lines = src.split(NL)
    items, i, n, run, depth = [], 0, len(lines), None, 0
    while i < n:
        l = lines[i]
        if l.strip() == '':
            run = None; depth = 0; i += 1; continue
        if l.startswith('//') or l.startswith('#['):
            if run is None: run = i
            if l.startswith('#['):
                depth += (l.count('(') + l.count('[') + l.count('{')) - (l.count(')') + l.count(']') + l.count('}'))
            i += 1; continue
        if run is not None and depth > 0:
            # indented continuation of an open #[...] group; still part of the attr block
            depth += (l.count('(') + l.count('[') + l.count('{')) - (l.count(')') + l.count(']') + l.count('}'))
            i += 1; continue
        m = START.match(l)
        if not m:
            run = None; depth = 0; i += 1; continue
        start = run if run is not None else i
        run = None
        depth = 0
        kind = m.group(5)
        nm = NAME.search(l)
        name = nm.group(1) if nm else (l.strip()[:60] if kind == 'impl' else '')
        one_line = l.rstrip().endswith(';') or (l.rstrip().endswith('}') and l.count('{') == l.count('}'))
        if one_line:
            items.append((start, i, kind, name)); i += 1; continue
        j = i
        while j < n:
            if j > i and lines[j].rstrip() in ('}', '};', ');', ')'):
                break
            j += 1
        items.append((start, j, kind, name)); i = j + 1
    return items

if __name__ == '__main__':
    text = open(sys.argv[1]).read()
    its = index(text)
    print('# items=%d covered=%d total=%d' % (len(its), sum(e - s + 1 for s, e, _, _ in its), len(text.splitlines())), file=sys.stderr)
    for s, e, k, nm in its:
        print('%d-%d%s%d%s%s%s%s' % (s + 1, e + 1, chr(9), e - s + 1, chr(9), k, chr(9), nm))
