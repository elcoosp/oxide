//! W3C Trace Context (spec §20: "W3C trace context propagated into `fetch` op
//! calls and user code").
//!
//! Parses and emits the `traceparent` header per the W3C Trace Context
//! recommendation. The `tracestate` header is passed through verbatim.
//!
//! The pipeline extracts a `TraceContext` from incoming request headers at
//! the start of `serve_request` and attaches it to the request-scoped span so
//! the tracing layer (when an OTLP endpoint is configured) emits the same
//! trace id. The `fetch` op reads the current context and adds a `traceparent`
//! header to outgoing requests.
//!
//! Note: the `fetch` op itself is a stub today (§4.5 — the V8 host does not
//! yet expose streaming ops), so the context is *available* but not yet
//! *consumed* by any live call site. This module exists so the op's
//! implementation lands with the format already stable.

use std::collections::BTreeMap;

/// A parsed W3C Trace Context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    /// 32 hex chars (16 bytes).
    pub trace_id: String,
    /// 16 hex chars (8 bytes).
    pub parent_id: String,
    /// Sampled bit of the trace flags byte.
    pub sampled: bool,
    /// The `tracestate` header verbatim, if present.
    pub tracestate: Option<String>,
}

impl TraceContext {
    /// Parse a `traceparent` header value. Returns `None` when the value is
    /// malformed or a reserved component. The format is:
    /// `00-<32 hex trace id>-<16 hex span id>-<2 hex flags>`.
    pub fn parse_traceparent(value: &str) -> Option<Self> {
        let parts: Vec<&str> = value.trim().split('-').collect();
        if parts.len() != 4 {
            return None;
        }
        let [version, trace_id, parent_id, flags] = [parts[0], parts[1], parts[2], parts[3]];
        // Version must be exactly two hex chars. `ff` is forbidden.
        if version.len() != 2 || !is_hex(version) || version == "ff" {
            return None;
        }
        if trace_id.len() != 32 || !is_hex(trace_id) || all_zero(trace_id) {
            return None;
        }
        if parent_id.len() != 16 || !is_hex(parent_id) || all_zero(parent_id) {
            return None;
        }
        if flags.len() != 2 || !is_hex(flags) {
            return None;
        }
        let sampled = u8::from_str_radix(flags, 16).map(|f| f & 0x01 != 0).unwrap_or(false);
        Some(TraceContext {
            trace_id: trace_id.to_string(),
            parent_id: parent_id.to_string(),
            sampled,
            tracestate: None,
        })
    }

    /// Extract a context from a header map. `tracestate`, if present, is
    /// carried through verbatim. Both keys are case-insensitive per HTTP.
    pub fn from_headers(headers: &BTreeMap<String, String>) -> Option<Self> {
        let mut found: Option<Self> = None;
        for (k, v) in headers {
            match k.to_ascii_lowercase().as_str() {
                "traceparent" => found = Self::parse_traceparent(v),
                _ => {}
            }
        }
        if let Some(mut tc) = found {
            for (k, v) in headers {
                if k.eq_ignore_ascii_case("tracestate") {
                    tc.tracestate = Some(v.clone());
                }
            }
            Some(tc)
        } else {
            None
        }
    }

    /// Emit a `traceparent` value using `self.trace_id` and the provided
    /// `span_id` (which the caller generates per outgoing request).
    pub fn to_traceparent(&self, span_id: &str) -> Option<String> {
        if span_id.len() != 16 || !is_hex(span_id) {
            return None;
        }
        let flags = if self.sampled { "01" } else { "00" };
        Some(format!("00-{}-{}-{}", self.trace_id, span_id, flags))
    }
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn all_zero(s: &str) -> bool {
    s.bytes().all(|b| b == b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_traceparent() {
        let tc = TraceContext::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )
        .unwrap();
        assert_eq!(tc.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(tc.parent_id, "b7ad6b7169203331");
        assert!(tc.sampled);
    }

    #[test]
    fn rejects_all_zero_trace_id() {
        assert!(TraceContext::parse_traceparent(
            "00-00000000000000000000000000000000-b7ad6b7169203331-01"
        )
        .is_none());
    }

    #[test]
    fn rejects_version_ff() {
        assert!(TraceContext::parse_traceparent(
            "ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        )
        .is_none());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(TraceContext::parse_traceparent("00-abc-def-01").is_none());
        assert!(TraceContext::parse_traceparent("00").is_none());
    }

    #[test]
    fn sampled_bit_false() {
        let tc = TraceContext::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00",
        )
        .unwrap();
        assert!(!tc.sampled);
    }

    #[test]
    fn from_headers_reads_case_insensitively() {
        let mut h = BTreeMap::new();
        h.insert(
            "TraceParent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        );
        h.insert("TraceState".to_string(), "vendor=val".to_string());
        let tc = TraceContext::from_headers(&h).unwrap();
        assert_eq!(tc.tracestate.as_deref(), Some("vendor=val"));
    }

    #[test]
    fn roundtrips_through_to_traceparent() {
        let tc = TraceContext::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )
        .unwrap();
        let out = tc.to_traceparent("b7ad6b7169203332").unwrap();
        assert_eq!(
            out,
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203332-01"
        );
        // Round-trip parse.
        let back = TraceContext::parse_traceparent(&out).unwrap();
        assert_eq!(back.trace_id, tc.trace_id);
        assert_eq!(back.parent_id, "b7ad6b7169203332");
    }

    #[test]
    fn to_traceparent_rejects_bad_span_id() {
        let tc = TraceContext::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )
        .unwrap();
        assert!(tc.to_traceparent("xyz").is_none());
    }
}
