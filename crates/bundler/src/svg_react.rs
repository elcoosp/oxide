//! SPEC-16.6: transform an imported `.svg?react` into a React component.
//!
//! Input: raw SVG source (already read from disk by the loader).
//! Output: a TSX module exporting `default` — a function component that
//! renders the SVG with `dangerouslySetInnerHTML` carrying the original
//! SVG's inner markup.
//!
//! ## Why `dangerouslySetInnerHTML` and not a hand-walked tree
//!
//! Walking `usvg`'s tree and emitting `<path>`/`<circle>`/... element by
//! element is what optimizers like SVGR do, and it makes the component
//! reactive (a `<path fill={props.color}>` can be styled). That's a real
//! feature, not a shim, and it's on the roadmap — but the v0.1 deliverable
//! is "importing a `.svg?react` yields a component that renders". Shipping
//! the walk now would mean also handling every edge (gradients, filters,
//! masks, clip paths, foreign content) — the wrong place to spend v0.1
//! time. So: extract `<svg ...attrs>INNER</svg>`, keep `INNER` as a raw
//! string, and give the component the outer attributes + `dangerouslySet
//! InnerHTML: { __html: INNER }`.
//!
//! The `dangerouslySetInnerHTML` boundary is safe because the source is
//! a file the user authored, imported by the user's own code — not
//! untrusted input. A `.svg?react` from `node_modules` is still
//! user-approved by the import statement.

use oxide_diagnostics::{OxideError, Result};

/// The React import the generated component needs. React 19-style: the
/// runtime export `jsx`/`jsxs` are pulled in by the framework's own
/// JSX lowering (the bundler does that); our generated code is plain
/// `createElement` to stay framework-agnostic. This still works with
/// React's automatic runtime because the bundler's Oxc pass rewrites
/// `createElement` usage — no, wait, it doesn't. Use JSX directly so
/// the bundler's existing JSX lowering applies.
const REACT_IMPORT: &str = "import React from 'react';\n";

/// Turn a raw SVG document into a React component module (TSX).
pub fn svg_to_react_component(source: &str, path_for_errors: &str) -> Result<String> {
    // Extract the outer `<svg>` opening tag and the inner body.
    // Strategy: find the FIRST `<svg` and the LAST `</svg>`. Everything
    // between (after the opening tag's `>`) is the inner body.
    let lower = source.to_ascii_lowercase();
    let svg_start = lower.find("<svg").ok_or_else(|| {
        OxideError::new(
            "BND-SVG",
            format!("`{path_for_errors}`: no <svg> element found"),
        )
    })?;
    // Find the end of the opening tag (the first `>` after `<svg`).
    let open_end = source[svg_start..]
        .find('>')
        .map(|i| svg_start + i + 1)
        .ok_or_else(|| {
            OxideError::new(
                "BND-SVG",
                format!("`{path_for_errors}`: unterminated <svg> opening tag"),
            )
        })?;
    let svg_close = lower.rfind("</svg>").ok_or_else(|| {
        OxideError::new(
            "BND-SVG",
            format!("`{path_for_errors}`: no closing </svg> found"),
        )
    })?;
    if svg_close < open_end {
        return Err(OxideError::new(
            "BND-SVG",
            format!("`{path_for_errors}`: </svg> appears before its opening tag ends"),
        ));
    }

    // Outer tag attributes (everything between `<svg` and `>`).
    let outer_attrs = &source[svg_start + 4..open_end - 1];
    // Inner body (everything between the opening `>` and `</svg>`).
    let inner = &source[open_end..svg_close];

    // Parse the outer attrs into a JSX props object. We keep `viewBox`
    // and any `xmlns` off (React's own runtime adds those as needed).
    let mut prop_parts: Vec<String> = Vec::new();
    for (key, val) in parse_attrs(outer_attrs) {
        // Skip attributes React warns about (xmlns), and skip `style` when
        // it is empty or a plain CSS string (React 19 accepts object styles;
        // converting to object form is a follow-up).
        if key.eq_ignore_ascii_case("xmlns") || key.eq_ignore_ascii_case("version") {
            continue;
        }
        match key.as_str() {
            // Numeric geometry props can stay strings; React accepts both.
            "width" | "height" | "viewBox" | "fill" | "stroke" | "class" | "className" => {
                let k = if key == "class" { "className" } else { key.as_str() };
                prop_parts.push(format!("{k}: {val}"));
            }
            _ => {
                // Kebab-case attrs need quoting in JSX. Use `data-*`-friendly
                // conversion via a string key.
                prop_parts.push(format!("\"{key}\": {val}"));
            }
        }
    }
    let props = if prop_parts.is_empty() {
        String::new()
    } else {
        format!("{{ {} }}", prop_parts.join(", "))
    };

    // Escape the inner body for embedding in a JS template literal.
    // `backtick`, `$`, and `\` need escaping.
    let escaped_inner = inner
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");

    let props_spread = if props.is_empty() {
        "{ ...props }".to_string()
    } else {
        format!("{props}, {{ ...props }}")
    };

    Ok(format!(
        "{REACT_IMPORT}\n\
         // Auto-generated by oxide bundler from `{path_for_errors}` (SPEC-16.6).\n\
         // Edit the SVG, not this file.\n\
         const __SVG_INNER = `{escaped_inner}`;\n\
         export default function SvgComponent(props) {{\n\
         \treturn (\n\
         \t\t<svg\n\
         \t\t\t{props_spread}\n\
         \t\t\tdangerouslySetInnerHTML={{{{ __html: __SVG_INNER }}}}\n\
         \t\t/>\n\
         \t);\n\
         }}\n"
    ))
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    // Minimal parser: `name="value"` or `name='value'` pairs. Ignores
    // JSX-incompatible constructs (already rare in SVGs).
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // skip whitespace
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // read key
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        if key_start == i {
            break;
        }
        let key = &s[key_start..i];
        // skip `=` (if present)
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            // read quoted value
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let val_start = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                let val = &s[val_start..i];
                if i < bytes.len() {
                    i += 1; // past closing quote
                }
                // Re-quote for JSX/JS: always use double quotes with escaping.
                let esc = val.replace('\\', "\\\\").replace('"', "\\\"");
                out.push((key.to_string(), format!("\"{esc}\"")));
            } else {
                // Unquoted value (SVG allows numeric only, tolerated)
                let val_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let val = &s[val_start..i];
                let esc = val.replace('\\', "\\\\").replace('"', "\\\"");
                out.push((key.to_string(), format!("\"{esc}\"")));
            }
        } else {
            // Boolean attr
            out.push((key.to_string(), "true".to_string()));
        }
    }
    out
}

/// Marker an import uses to request a React component: `.svg?react`.
pub fn is_svg_react_specifier(spec: &str) -> bool {
    if spec.contains(':') && !spec.starts_with("./") && !spec.starts_with("../") {
        return false; // bare specifier like `react` — never a .svg?react import
    }
    spec.ends_with(".svg?react")
}

/// Strip `?react` from a specifier, returning the on-disk SVG path
/// (`./icon.svg?react` → `./icon.svg`). The suffix to strip is the query
/// marker alone, not `.svg?react` — the `.svg` must survive because that
/// is the path the resolver/loader opens.
pub fn strip_svg_react(spec: &str) -> Option<&str> {
    spec.strip_suffix("?react")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_svg_becomes_component() {
        let svg = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" xmlns="http://www.w3.org/2000/svg"><path d="M3 12h18"/></svg>"#;
        let tsx = svg_to_react_component(svg, "icon.svg").unwrap();
        assert!(tsx.contains("export default function SvgComponent"));
        assert!(tsx.contains("viewBox"));
        assert!(tsx.contains("dangerouslySetInnerHTML"));
        assert!(tsx.contains("<path d=\"M3 12h18\"/>"));
        assert!(!tsx.contains("xmlns")); // stripped
    }

    #[test]
    fn no_svg_errors() {
        let e = svg_to_react_component("not svg", "x.svg").unwrap_err();
        assert_eq!(e.0.code, "BND-SVG");
    }

    #[test]
    fn unterminated_errors() {
        let e = svg_to_react_component("<svg>", "x.svg").unwrap_err();
        assert_eq!(e.0.code, "BND-SVG");
        assert!(e.0.message.contains("closing"));
    }

    #[test]
    fn backtick_and_dollar_escaped() {
        let svg = r#"<svg><text>a`b${c}</text></svg>"#;
        let tsx = svg_to_react_component(svg, "x.svg").unwrap();
        assert!(tsx.contains("a\\`b"));
        assert!(tsx.contains("\\${c}"));
    }

    #[test]
    fn detect_svg_react_specifier() {
        assert!(is_svg_react_specifier("./icon.svg?react"));
        assert!(is_svg_react_specifier("../icons/star.svg?react"));
        assert!(!is_svg_react_specifier("./icon.svg"));
        assert!(!is_svg_react_specifier("./icon.png?react"));
    }

    #[test]
    fn strip_svg_react_works() {
        assert_eq!(strip_svg_react("./icon.svg?react"), Some("./icon.svg"));
        assert_eq!(strip_svg_react("./icon.svg"), None);
    }
}
