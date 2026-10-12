//! CSS helpers (spec §6.6, D5). The actual CSS pipeline is `LightningCssBackend`
//! (lightningcss); this module retains only the Tailwind candidate scanner,
//! which feeds the `@tailwindcss/oxide` invocation through the tooling pool.

use std::collections::BTreeSet;

/// Tailwind v4 candidate scanner (§6.6): class-like tokens from `class`/`className`/
/// `class:list`/`:class` attribute values. Split on whitespace, deduped and sorted.
/// Only attribute values are considered — utility-looking words in plain code/text
/// are NOT candidates (the engine itself lives in the tailwind plugin).
pub fn scan_tailwind_candidates(sources: &[&str]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for src in sources {
        scan_source(src, &mut set);
    }
    set.into_iter().collect()
}

fn scan_source(src: &str, out: &mut std::collections::BTreeSet<String>) {
    const ATTRS: [&str; 4] = ["class=", "className=", "class:list=", ":class="];
    for attr in ATTRS {
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(attr) {
            let vstart = from + rel + attr.len();
            let bytes = src.as_bytes();
            let Some(&q) = bytes.get(vstart) else { break };
            if q != b'"' && q != b'\'' {
                from = vstart.max(from + 1);
                continue;
            }
            let Some(end_rel) = src[vstart + 1..].find(q as char) else {
                break;
            };
            let vend = vstart + 1 + end_rel;
            let mut val = &src[vstart + 1..vend];
            // Vue/Astro expression form: :class="'a b'" — strip one layer of quotes.
            if val.len() >= 2
                && ((val.starts_with('\'') && val.ends_with('\''))
                    || (val.starts_with('"') && val.ends_with('"')))
            {
                val = &val[1..val.len() - 1];
            }
            for tok in val.split_whitespace() {
                out.insert(tok.to_string());
            }
            from = vend + 1;
            if from >= src.len() {
                break;
            }
        }
    }
}
