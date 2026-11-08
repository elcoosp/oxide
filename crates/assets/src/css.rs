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

#[cfg(test)]
mod tests {
    use super::scan_tailwind_candidates;

    /// The scanner feeds `@tailwindcss/oxide` through the tooling pool: a
    /// missed token produces missing CSS utilities; an over-scan bloats the
    /// CSS. Both directions are regressions, so the contract is pinned from
    /// both sides: real markup shapes (JSX/Vue/Svelte/Astro) must yield their
    /// utility tokens, and utility-looking words OUTSIDE class attributes
    /// must stay out (the engine is the tailwind plugin, not this scanner).
    #[test]
    fn jsx_class_name_yields_tokens() {
        let out = scan_tailwind_candidates(&["<div className=\"flex items-center gap-2\"></div>"]);
        assert_eq!(out, vec!["flex", "gap-2", "items-center"]);
    }

    #[test]
    fn html_class_attribute_yields_tokens() {
        let out = scan_tailwind_candidates(&["<p class=\"prose dark:prose-invert\"></p>"]);
        assert_eq!(out, vec!["dark:prose-invert", "prose"]);
    }

    #[test]
    fn single_quoted_attribute_yields_tokens() {
        let out = scan_tailwind_candidates(&["<x class='p-4 m-2'></x>"]);
        assert_eq!(out, vec!["m-2", "p-4"]);
    }

    #[test]
    fn multiple_sources_merge_deduped_and_sorted() {
        let out =
            scan_tailwind_candidates(&["<div class=\"b a\"></div>", "<span class=\"a c\"></span>"]);
        assert_eq!(
            out,
            vec!["a", "b", "c"],
            "BTreeSet: deduped + sorted output"
        );
    }

    #[test]
    fn vue_expression_form_strips_the_inner_quote_layer() {
        // :class="'a b'" — the value is itself a quoted string inside the attr.
        let out = scan_tailwind_candidates(&[":class=\"'w-full h-4'\""]);
        assert_eq!(out, vec!["h-4", "w-full"]);
    }

    #[test]
    fn svelte_class_directive_value_is_scanned() {
        let out = scan_tailwind_candidates(&["class:list=\"rounded-lg shadow\""]);
        assert_eq!(out, vec!["rounded-lg", "shadow"]);
    }

    #[test]
    fn utility_looking_words_outside_class_attributes_are_ignored() {
        // `class` inside code/text must NOT become a candidate: only
        // attribute values feed the scanner (§6.6).
        let out = scan_tailwind_candidates(&[
            "let class = \"flex\"; // not an attribute",
            "<p>learn flex in 10 minutes</p>",
            "const className2 = 'grid'",
        ]);
        assert!(out.is_empty(), "got {out:?}");
    }

    #[test]
    fn unquoted_attribute_value_is_not_a_candidate() {
        // `<div class=flex>` — no quote: ambiguous and not the emitted markup
        // shape; the scanner must skip it rather than guess a token boundary.
        let out = scan_tailwind_candidates(&["<div class=flex></div>"]);
        assert!(out.is_empty(), "got {out:?}");
    }

    #[test]
    fn unbalanced_quote_yields_no_candidates_and_does_not_hang() {
        // An unterminated attribute has no reliable end boundary, so the
        // scanner contributes NOTHING from that attribute (and stops scanning
        // it) rather than guessing a token boundary over the rest of the file.
        // The non-hang property is the §22 regression class this pins.
        let out = scan_tailwind_candidates(&["<div class=\"pt-2 and then no close"]);
        assert!(out.is_empty(), "got {out:?}");
        // Scanning terminates on a large pathological input.
        let big = format!("<div class=\"{}>", "x ".repeat(50_000));
        let out = scan_tailwind_candidates(&[big.as_str()]);
        assert!(out.is_empty(), "got {out:?}");
    }

    #[test]
    fn adjacent_attributes_each_yield_their_tokens() {
        let src = "<div class=\"a1\" data-x=\"junk\" className=\"b2\"></div>";
        let out = scan_tailwind_candidates(&[src]);
        assert_eq!(
            out,
            vec!["a1", "b2"],
            "data-x must not leak into candidates"
        );
    }

    #[test]
    fn empty_and_whitespace_only_values_yield_nothing() {
        let out = scan_tailwind_candidates(&["<div class=\"   \"></div>", "", "<i class=''></i>"]);
        assert!(out.is_empty());
    }

    #[test]
    fn unicode_and_dash_tokens_survive_verbatim() {
        let out = scan_tailwind_candidates(&["<div class=\"w-1/2 -translate-x-1/2\"></div>"]);
        assert_eq!(out, vec!["-translate-x-1/2", "w-1/2"]);
    }

    #[test]
    fn repeated_class_attributes_in_one_source_all_get_scanned() {
        let src = "<div class=\"mt-4\"><span class=\"ml-2\"></span></div>";
        let out = scan_tailwind_candidates(&[src]);
        assert_eq!(out, vec!["ml-2", "mt-4"]);
    }
}
