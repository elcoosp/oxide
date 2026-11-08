# benchmarks/criterion_summary.py <out.json> — V2-B09: median ns per benchmark
# from target/criterion. Run after `cargo bench` to produce the JSON that
# `criterion_compare.py` diffs against the committed baseline.
import json, glob, os, sys

out = {}
for p in glob.glob('target/criterion/**/new/estimates.json', recursive=True):
    name = os.path.relpath(os.path.dirname(os.path.dirname(p)), 'target/criterion')
    out[name] = json.load(open(p))['median']['point_estimate']
json.dump(out, open(sys.argv[1], 'w'), indent=1, sort_keys=True)
print(len(out), 'benchmarks')
