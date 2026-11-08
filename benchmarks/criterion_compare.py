# benchmarks/criterion_compare.py <baseline.json> <new.json> [max_regress_percent=5]
# V2-B09: compare two criterion summaries. Improvements show as negative
# percentages; only regressions fail (exit 1). Benchmarks missing from either
# file are reported but never gate.
import json, sys

b, n = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
limit = float(sys.argv[3]) if len(sys.argv) > 3 else 5.0
bad = 0
missing = [k for k in sorted(b) if k not in n]
if missing:
    print('%d baseline benchmark(s) missing from the new run (info only):' % len(missing))
    for k in missing:
        print('  - %s' % k)
for k in sorted(b):
    if k in n and b[k] > 0:
        pct = (n[k] - b[k]) / b[k] * 100
        flag = 'REGRESSION' if pct > limit else ''
        bad += 1 if flag else 0
        print('%-60s %12.1f -> %12.1f  %+7.1f%% %s' % (k, b[k], n[k], pct, flag))
sys.exit(1 if bad else 0)
