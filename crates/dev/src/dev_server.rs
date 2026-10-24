//! Dev server extras (spec §13.5): built-in HTTPS with a locally-trusted
//! certificate, network URL display, QR code, proxy rules, and browser open.
//!
//! The dev server itself lives in `oxide-fw-server`; this module provides the
//! pieces that only apply in development:
//!
//! - `generate_dev_cert` — a self-signed certificate covering `localhost`,
//!   `127.0.0.1`, `::1`, and every local IP, so a phone on the LAN gets a
//!   hostname-matching cert (browser still warns about the CA — that is
//!   unavoidable without a system-installed root; the alternative is `mkcert`,
//!   which users can plug in via `DevServerConfig::tls` overrides).
//! - `network_url` — the LAN-reachable URL (`http://192.168.x.y:3000`).
//! - `render_qr` — a Unicode half-block QR code for the terminal.
//! - `ProxyRule` / `apply_proxy` — prefix-matched reverse proxy rules.
//! - `open_browser` — OS-level URL opener.

use crate::{Result, err};
use std::net::IpAddr;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Everything the dev server needs to know about its extra features.
#[derive(Debug, Clone, Default)]
pub struct DevServerConfig {
    /// Enable HTTPS with a self-signed cert.
    pub https: bool,
    /// Bind to all interfaces (0.0.0.0) instead of loopback.
    pub lan: bool,
    /// Print the network URL as a QR code.
    pub qr_code: bool,
    /// Open the browser on start.
    pub open_browser: bool,
    /// Proxy rules, evaluated longest-prefix first.
    pub proxy: Vec<ProxyRule>,
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

/// A generated certificate + private key in PEM form, ready for rustls.
#[derive(Debug, Clone)]
pub struct DevCert {
    pub cert_pem: String,
    pub key_pem: String,
    /// The subject alternative names the cert covers.
    pub sans: Vec<String>,
}

/// Generate a self-signed certificate valid for `localhost`, loopback
/// addresses, and every currently-detected LAN IP. Uses `rcgen` with the
/// `ring` backend (already in the workspace).
///
/// On macOS/iOS, browsers will show a warning until the user trusts the cert
/// (or uses a tool like `mkcert`); on Linux, some distros accept self-signed
/// certs for `localhost` without a warning.
pub fn generate_dev_cert() -> Result<DevCert> {
    let mut sans: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Ok(ip) = local_ip_address::local_ip() {
        let s = ip.to_string();
        if !sans.contains(&s) {
            sans.push(s);
        }
    }
    if let Ok(list) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in list {
            // Skip link-local and loopback (already covered).
            let s = ip.to_string();
            if sans.contains(&s) {
                continue;
            }
            if ip.is_loopback() {
                continue;
            }
            if let IpAddr::V4(v4) = ip {
                if v4.is_link_local() {
                    continue;
                }
            }
            sans.push(s);
        }
    }

    let params = rcgen::CertificateParams::new(sans.clone())
        .map_err(|e| err("DEV-TLS", format!("certificate params failed: {e}")))?;
    let key = rcgen::KeyPair::generate()
        .map_err(|e| err("DEV-TLS", format!("key generation failed: {e}")))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| err("DEV-TLS", format!("self-sign failed: {e}")))?;

    Ok(DevCert {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
        sans,
    })
}

// ---------------------------------------------------------------------------
// Network URL
// ---------------------------------------------------------------------------

/// The LAN-reachable URL for `port`, or `None` if no non-loopback interface
/// exists (offline machine, VPN-only host, etc.).
pub fn network_url(port: u16, https: bool) -> Option<String> {
    let ip = local_ip_address::local_ip().ok()?;
    let scheme = if https { "https" } else { "http" };
    Some(format!("{scheme}://{ip}:{port}"))
}

/// The loopback URL for `port`.
pub fn local_url(port: u16, https: bool) -> String {
    let scheme = if https { "https" } else { "http" };
    format!("{scheme}://localhost:{port}")
}

// ---------------------------------------------------------------------------
// QR code
// ---------------------------------------------------------------------------

/// Render `data` as a Unicode half-block QR code, suitable for printing to a
/// terminal. Uses `▀`/`▄`/`█` so each character cell carries two QR rows.
///
/// The quiet zone is included (8 modules on each side) because scanners rely
/// on it for synchronisation.
pub fn render_qr(data: &str) -> Result<String> {
    let code = qrcode::QrCode::new(data.as_bytes())
        .map_err(|e| err("DEV-QR", format!("QR generation failed: {e}")))?;
    let colors = code.to_colors();
    let width = code.width();
    let quiet = 2; // 2 character cells = 4 QR rows of padding

    let mut out = String::new();
    // Top padding rows.
    let blank_row = " ".repeat(width + quiet * 2);
    for _ in 0..quiet {
        out.push_str(&blank_row);
        out.push('\n');
    }
    // QR rows in pairs: each output row renders two QR rows using half blocks.
    let mut y = 0;
    while y < width {
        for _ in 0..quiet {
            out.push(' ');
        }
        for x in 0..width {
            let top = colors[y * width + x] == qrcode::types::Color::Dark;
            let bottom = if y + 1 < width {
                colors[(y + 1) * width + x] == qrcode::types::Color::Dark
            } else {
                false
            };
            let ch = match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            out.push(ch);
        }
        for _ in 0..quiet {
            out.push(' ');
        }
        out.push('\n');
        y += 2;
    }
    // Bottom padding rows.
    for _ in 0..quiet {
        out.push_str(&blank_row);
        out.push('\n');
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Proxy rules
// ---------------------------------------------------------------------------

/// A single reverse-proxy rule (§13.5). Match by longest-prefix wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRule {
    /// Path prefix, e.g. `/api`.
    pub prefix: String,
    /// Upstream target origin, e.g. `http://localhost:4000`.
    pub target: String,
    /// Strip the prefix before forwarding (common for API gateways).
    pub strip_prefix: bool,
}

impl ProxyRule {
    pub fn new(prefix: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            target: target.into(),
            strip_prefix: false,
        }
    }

    pub fn with_strip(mut self, strip: bool) -> Self {
        self.strip_prefix = strip;
        self
    }
}

/// The result of applying proxy rules to a request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyMatch {
    /// Full URL to forward to (target + rewritten path + query).
    pub url: String,
    /// The rule that matched, for logging.
    pub rule: ProxyRule,
}

/// Apply `rules` to `path` (with optional `query` — passed verbatim).
/// Returns `None` when no rule matches; the caller serves locally.
///
/// Matching is by **longest prefix** so `/api/v2` can override `/api`.
/// A prefix matches when it equals the path or is followed by `/`
/// (so `/api` matches `/api/users` but not `/apixyz`).
pub fn apply_proxy(rules: &[ProxyRule], path: &str, query: Option<&str>) -> Option<ProxyMatch> {
    let mut best: Option<&ProxyRule> = None;
    for rule in rules {
        if !prefix_matches(&rule.prefix, path) {
            continue;
        }
        match best {
            Some(prev) if prev.prefix.len() >= rule.prefix.len() => {}
            _ => best = Some(rule),
        }
    }
    let rule = best?;

    let target = rule.target.trim_end_matches('/');
    let path_part = if rule.strip_prefix {
        path.strip_prefix(&rule.prefix).unwrap_or(path)
    } else {
        path
    };
    let path_part = if path_part.is_empty() { "/" } else { path_part };
    let mut url = format!("{target}{path_part}");
    if let Some(q) = query {
        if !q.is_empty() {
            url.push('?');
            url.push_str(q);
        }
    }
    Some(ProxyMatch {
        url,
        rule: rule.clone(),
    })
}

fn prefix_matches(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    let prefix = prefix.trim_end_matches('/');
    path == prefix
        || path
            .strip_prefix(prefix)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Browser open
// ---------------------------------------------------------------------------

/// Open `url` in the user's default browser. On failure (headless CI,
/// missing `xdg-open`, etc.) returns `Ok(false)` rather than an error — this
/// is a convenience, not a requirement.
pub fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let cmd = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let cmd = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let cmd: (&str, Vec<&str>) = ("true", vec![]);

    std::process::Command::new(cmd.0)
        .args(&cmd.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- cert --------------------------------------------------------------

    #[test]
    fn dev_cert_is_pem_and_covers_localhost() {
        let c = generate_dev_cert().unwrap();
        assert!(c.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(c.key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(c.sans.contains(&"localhost".to_string()));
        assert!(c.sans.contains(&"127.0.0.1".to_string()));
    }

    #[test]
    fn dev_cert_is_deterministic_in_shape_across_calls() {
        // Two calls produce distinct keys (RNG) but the same SAN set.
        let a = generate_dev_cert().unwrap();
        let b = generate_dev_cert().unwrap();
        assert_eq!(a.sans, b.sans);
        assert_ne!(a.key_pem, b.key_pem);
    }

    // ---- URLs --------------------------------------------------------------

    #[test]
    fn local_url_shape() {
        assert_eq!(local_url(3000, false), "http://localhost:3000");
        assert_eq!(local_url(3000, true), "https://localhost:3000");
    }

    #[test]
    fn network_url_is_scheme_correct_when_present() {
        if let Some(u) = network_url(3000, false) {
            assert!(u.starts_with("http://"), "{u}");
            assert!(u.ends_with(":3000"), "{u}");
        }
        if let Some(u) = network_url(3000, true) {
            assert!(u.starts_with("https://"), "{u}");
        }
    }

    // ---- QR ----------------------------------------------------------------

    #[test]
    fn qr_renders_multiple_lines_of_block_characters() {
        let out = render_qr("http://192.168.1.5:3000").unwrap();
        assert!(out.lines().count() > 5);
        assert!(out.contains('█') || out.contains('▀') || out.contains('▄'));
    }

    #[test]
    fn qr_is_deterministic_for_same_input() {
        let a = render_qr("http://x").unwrap();
        let b = render_qr("http://x").unwrap();
        assert_eq!(a, b);
    }

    // ---- proxy -------------------------------------------------------------

    #[test]
    fn proxy_matches_prefix_at_segment_boundary() {
        let rules = vec![ProxyRule::new("/api", "http://localhost:4000")];
        // Matches
        assert!(apply_proxy(&rules, "/api", None).is_some());
        assert!(apply_proxy(&rules, "/api/users", None).is_some());
        // Does not match (prefix bleeds into a longer segment)
        assert!(apply_proxy(&rules, "/apixyz", None).is_none());
        assert!(apply_proxy(&rules, "/other", None).is_none());
    }

    #[test]
    fn proxy_longest_prefix_wins() {
        let rules = vec![
            ProxyRule::new("/api", "http://old"),
            ProxyRule::new("/api/v2", "http://v2"),
        ];
        let m = apply_proxy(&rules, "/api/v2/users", None).unwrap();
        assert_eq!(m.rule.target, "http://v2");
        assert_eq!(m.url, "http://v2/api/v2/users");
    }

    #[test]
    fn proxy_strip_prefix() {
        let rules = vec![ProxyRule::new("/api", "http://x:4000").with_strip(true)];
        let m = apply_proxy(&rules, "/api/users", None).unwrap();
        assert_eq!(m.url, "http://x:4000/users");
        // Root case: stripping the whole path → "/".
        let m2 = apply_proxy(&rules, "/api", None).unwrap();
        assert_eq!(m2.url, "http://x:4000/");
    }

    #[test]
    fn proxy_preserves_query() {
        let rules = vec![ProxyRule::new("/api", "http://x:4000")];
        let m = apply_proxy(&rules, "/api/users", Some("page=2&limit=10")).unwrap();
        assert_eq!(m.url, "http://x:4000/api/users?page=2&limit=10");
    }

    #[test]
    fn proxy_target_trailing_slash_is_normalised() {
        let rules = vec![ProxyRule::new("/api", "http://x:4000/")];
        let m = apply_proxy(&rules, "/api/users", None).unwrap();
        assert_eq!(m.url, "http://x:4000/api/users");
    }

    #[test]
    fn proxy_no_rules_returns_none() {
        assert!(apply_proxy(&[], "/any", None).is_none());
    }
}
