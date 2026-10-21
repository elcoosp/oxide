//! Partial prerendering (spec §10.4). The static shell up to each deferred
//! boundary is cached in L0; only the holes render per request and stream into
//! the cached shell. Because boundaries are core-defined (§7.1), PPR is
//! framework-neutral: the shell+cache-key format is identical for all four
//! adapters.
//!
//! The protocol:
//! 1. `split_shell(html)` walks the placeholder markers emitted by the core
//!    boundary protocol (`<template data-ox-b="ID">…</template>`) and returns
//!    the shell with each hole replaced by an opaque slot reference, plus the
//!    list of hole IDs.
//! 2. The shell (deterministic given route + build id) is cached in L0.
//! 3. Per request, `assemble(shell, resolved_holes)` substitutes the resolved
//!    boundary content streams back into the cached shell and yields a single
//!    streaming response.

use crate::{Result, err};
use std::collections::BTreeMap;

/// The static shell of a PPR response — the HTML with each deferred boundary
/// replaced by a stable `<ox-hole id="…"></ox-hole>` placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// HTML with holes as `<ox-hole id="ID"></ox-hole>` placeholders.
    pub html: String,
    /// IDs of the holes, in document order.
    pub hole_ids: Vec<String>,
}

/// Split a fully-rendered page (with `data-ox-b` boundaries) into a cacheable
/// shell and a list of hole IDs.
pub fn split_shell(html: &str) -> Result<Shell> {
    let mut out = String::with_capacity(html.len());
    let mut holes = Vec::new();
    let mut cursor = 0usize;
    while let Some(open_rel) = html[cursor..].find("<template data-ox-b=") {
        let open = cursor + open_rel;
        // Copy everything before the boundary verbatim.
        out.push_str(&html[cursor..open]);
        // Parse the ID: `<template data-ox-b="ID">` where the quote may be
        // single or double.
        let after = &html[open + "<template data-ox-b=".len()..];
        let quote = after
            .chars()
            .next()
            .ok_or_else(|| err("RND-PPR", "unterminated boundary tag"))?;
        if quote != '"' && quote != '\'' {
            return Err(err(
                "RND-PPR",
                format!("boundary marker must quote the id, saw `{quote}`"),
            ));
        }
        let rest = &after[1..];
        let close_q = rest
            .find(quote)
            .ok_or_else(|| err("RND-PPR", "unclosed boundary id quote"))?;
        let id = rest[..close_q].to_string();
        // Find the matching `</template>` after this opening tag.
        let after_open = &rest[close_q + 1..];
        let close_rel = after_open
            .find("</template>")
            .ok_or_else(|| err("RND-PPR", format!("boundary `{id}` has no closing tag")))?;
        // Emit an opaque placeholder in the shell.
        out.push_str(&format!("<ox-hole id=\"{id}\"></ox-hole>"));
        holes.push(id);
        cursor =
            open + "<template data-ox-b=".len() + 1 + close_q + 1 + close_rel + "</template>".len();
    }
    // Trailing content after the last boundary.
    out.push_str(&html[cursor..]);
    Ok(Shell {
        html: out,
        hole_ids: holes,
    })
}

/// Assemble the final HTML from a cached shell and the per-request resolved
/// boundary content. `resolved` maps hole id → resolved HTML.
pub fn assemble(shell: &Shell, resolved: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::with_capacity(shell.html.len() + 256);
    let mut cursor = 0usize;
    while let Some(open_rel) = shell.html[cursor..].find("<ox-hole id=\"") {
        let open = cursor + open_rel;
        out.push_str(&shell.html[cursor..open]);
        let after = &shell.html[open + "<ox-hole id=\"".len()..];
        let close_q = after
            .find('"')
            .ok_or_else(|| err("RND-PPR", "unterminated hole id quote"))?;
        let id = &after[..close_q];
        let after_q = &after[close_q + 1..];
        // Skip `></ox-hole>`.
        let end_rel = after_q
            .find("></ox-hole>")
            .ok_or_else(|| err("RND-PPR", format!("hole `{id}` has no closing marker")))?;
        let fill = resolved.get(id).cloned().unwrap_or_else(|| String::new());
        out.push_str(&fill);
        cursor = open + "<ox-hole id=\"".len() + close_q + 1 + end_rel + "></ox-hole>".len();
    }
    out.push_str(&shell.html[cursor..]);
    Ok(out)
}

/// §10.8: the client half of PPR. A cached shell carries `<ox-hole id="…">`
/// placeholders; the live render streams `<template data-ox-c="…">fragment
/// </template>` chunks for each resolved boundary (the B-03 stream). This
/// resolver swaps each fragment into its hole as it arrives, and also walks
/// templates already present at parse time.
///
/// CSP-safe (nonce-carrying, no eval); ids are compared by attribute, never
/// interpolated into a selector.
pub fn client_resolver(nonce: &str) -> String {
    let mut out = String::with_capacity(900);
    out.push_str("<script nonce=\"");
    out.push_str(nonce);
    out.push_str("\">(function(){");
    out.push_str("function fill(id){var hs=document.querySelectorAll('ox-hole');");
    out.push_str("for(var i=0;i<hs.length;i++){var h=hs[i];if(h.getAttribute('id')!==id)continue;");
    out.push_str("var ts=document.querySelectorAll(\"template[data-ox-c]\");");
    out.push_str("for(var j=0;j<ts.length;j++){var t=ts[j];");
    out.push_str("if(t.getAttribute('data-ox-c')!==id)continue;");
    out.push_str("var d=document.createElement('div');d.innerHTML=t.innerHTML;");
    out.push_str("h.replaceWith.apply(h,d.childNodes);");
    out.push_str(
        "document.dispatchEvent(new CustomEvent('ox:swap',{detail:{id:id,value:t.innerHTML}}));",
    );
    out.push_str("return;}}}");
    out.push_str("function scan(){var ts=document.querySelectorAll(\"template[data-ox-c]\");");
    out.push_str(
        "for(var i=0;i<ts.length;i++){var id=ts[i].getAttribute('data-ox-c');if(id)fill(id);}}",
    );
    out.push_str("var mo=new MutationObserver(function(ms){for(var i=0;i<ms.length;i++){");
    out.push_str("var a=ms[i].addedNodes;for(var j=0;j<a.length;j++){var n=a[j];");
    out.push_str("if(n.nodeType===1&&n.tagName==='TEMPLATE'&&n.getAttribute('data-ox-c'))");
    out.push_str("fill(n.getAttribute('data-ox-c'));}}});");
    out.push_str("if(typeof document!=='undefined'&&document.documentElement){");
    out.push_str("mo.observe(document.documentElement,{childList:true,subtree:true});scan();}");
    out.push_str("})();</script>");
    out
}

/// Cache key for a shell (§10.4): route + params + build id + Vary dims. Any
/// static shell from the same build for the same route shares one entry.
pub fn shell_cache_key(
    route: &str,
    params: &BTreeMap<String, String>,
    build_id: &str,
    vary: &[(&str, &str)],
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"oxide-ppr-shell-v1\0");
    h.update(route.as_bytes());
    h.update(b"\0");
    for (k, v) in params {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(v.as_bytes());
        h.update(b"\0");
    }
    h.update(build_id.as_bytes());
    h.update(b"\0");
    for (k, v) in vary {
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(v.as_bytes());
        h.update(b"\0");
    }
    h.finalize().to_hex().to_string()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_shell_extracts_boundaries() {
        let html = "<html><body><h1>Hello</h1><template data-ox-b=\"c1\"><p>Loading</p></template><footer/></body></html>";
        let shell = split_shell(html).unwrap();
        assert_eq!(shell.hole_ids, vec!["c1"]);
        assert!(shell.html.contains("<ox-hole id=\"c1\"></ox-hole>"));
        assert!(shell.html.contains("<h1>Hello</h1>"));
        assert!(shell.html.contains("<footer/>"));
        assert!(!shell.html.contains("<p>Loading</p>"));
    }

    #[test]
    fn split_shell_handles_multiple_and_no_boundaries() {
        let html = "a<template data-ox-b=\"x\">X</template>b<template data-ox-b='y'>Y</template>c";
        let shell = split_shell(html).unwrap();
        assert_eq!(shell.hole_ids, vec!["x", "y"]);
        assert_eq!(
            shell.html,
            "a<ox-hole id=\"x\"></ox-hole>b<ox-hole id=\"y\"></ox-hole>c"
        );

        let plain = split_shell("<p>static</p>").unwrap();
        assert!(plain.hole_ids.is_empty());
        assert_eq!(plain.html, "<p>static</p>");
    }

    #[test]
    fn assemble_substitutes_resolved_holes() {
        let shell = Shell {
            html: "a<ox-hole id=\"x\"></ox-hole>b<ox-hole id=\"y\"></ox-hole>c".into(),
            hole_ids: vec!["x".into(), "y".into()],
        };
        let mut resolved = BTreeMap::new();
        resolved.insert("x".into(), "<p>1</p>".into());
        resolved.insert("y".into(), "<p>2</p>".into());
        let out = assemble(&shell, &resolved).unwrap();
        assert_eq!(out, "a<p>1</p>b<p>2</p>c");
    }

    #[test]
    fn assemble_leaves_unresolved_holes_empty() {
        let shell = Shell {
            html: "a<ox-hole id=\"x\"></ox-hole>b".into(),
            hole_ids: vec!["x".into()],
        };
        let out = assemble(&shell, &BTreeMap::new()).unwrap();
        assert_eq!(out, "ab");
    }

    #[test]
    fn shell_cache_key_is_deterministic_and_param_sensitive() {
        let p1 = BTreeMap::from([("id".to_string(), "1".to_string())]);
        let p2 = BTreeMap::from([("id".to_string(), "2".to_string())]);
        let a = shell_cache_key("/photos/:id", &p1, "build-1", &[]);
        let b = shell_cache_key("/photos/:id", &p1, "build-1", &[]);
        let c = shell_cache_key("/photos/:id", &p2, "build-1", &[]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn split_then_assemble_roundtrips_full_html() {
        let original = "pre<template data-ox-b=\"x\"><p>dynamic</p></template>post";
        let shell = split_shell(original).unwrap();
        let mut resolved = BTreeMap::new();
        resolved.insert("x".into(), "<p>dynamic</p>".into());
        let out = assemble(&shell, &resolved).unwrap();
        assert_eq!(out, "pre<p>dynamic</p>post");
    }
}
