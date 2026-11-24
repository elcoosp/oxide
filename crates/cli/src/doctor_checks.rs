//! §V2-E08 item 3's remaining doctor checks — the cache-handler
//! reachability probe (A10), the server-port availability probe, the
//! file-descriptor limit probe and the signed-cookie clock-skew probe.
//!
//! Table-driven, per the spec's own test contract: every check maps an
//! INJECTED fact (a probed value or a probe fn pointer) to a [`Verdict`] —
//! no check touches the live system directly, so the pins drive fakes and
//! `cmd_doctor` wires the real probes at the call site. A check reports
//! one [`Verdict`]: `Ok` prints `[ok]`, `Warn` prints `[warn]` (the
//! spec's "warns" — never a doctor failure), `Fail` prints `[fail]` and
//! flips the exit code, `Skip` prints `[--]` (informational, nothing
//! probed).

use oxide_fw_config::CacheHandlerConfig;

/// The signed-cookie clock-skew tolerance — the recorded default this
/// milestone documents (the spec names no tolerance; the cookie AEAD's
/// timestamp windows are minute-scale, so a minute of drift is the honest
/// line).
pub(crate) const CLOCK_SKEW_TOLERANCE_SECS: i64 = 60;

/// The fd-limit soft limit the doctor warns under (the spec's number —
/// a dev server holding sockets, watch roots and asset handles exhausts
/// the default 1024 long before a real app does).
pub(crate) const FD_LIMIT_WARN_UNDER: u64 = 4096;

/// One doctor check's verdict: how the row prints and whether it flips
/// the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// `[ok]`.
    Ok,
    /// `[warn]` — the spec's "warns": surfaced, never a doctor failure.
    Warn,
    /// `[fail]` — flips the doctor exit code.
    Fail,
    /// `[--]` — informational, nothing probed.
    Skip,
}

/// One check row: the printed detail (everything after the `[verdict]`
/// marker) and the verdict it carries.
pub(crate) struct CheckRow {
    pub what: String,
    pub verdict: Verdict,
}

/// A remote cache endpoint the doctor can probe: the scheme label (the
/// config spelling it came from) plus the host:port pair.
pub(crate) struct RemoteEndpoint {
    pub label: String,
    pub host: String,
    pub port: u16,
}

/// The injected network probes the reachability and port checks consult.
/// Fn pointers, so the pins pass non-capturing closures and `cmd_doctor`
/// passes the real probes — neither captures, neither blocks beyond its
/// own bounded connect.
pub(crate) struct DoctorProbes {
    /// TCP connect-ability: `(host, port) -> reachable`. The real probe
    /// is a 1-second-bounded `TcpStream::connect_timeout`.
    pub tcp_connect: fn(&str, u16) -> bool,
    /// Bind-ability: `(host, port) -> the listener binds`.
    pub try_bind: fn(&str, u16) -> bool,
}

/// Parse one `redis://`/`http(s)://` URL into a probeable endpoint: the
/// scheme names the label, the authority names the host:port pair (the
/// scheme's default port when the authority carries none). `None` for a
/// URL the parse cannot attribute — the check reports the raw spelling
/// rather than probing a guess.
fn endpoint_from_url(label: &str, url: &str, default_port: u16) -> Option<RemoteEndpoint> {
    let (scheme, authority) = url.split_once("://")?;
    // The scheme carries the secure spelling's own default (`https` → 443);
    // every other scheme falls back to the caller's (redis → 6379).
    let default_port = if scheme == "https" { 443 } else { default_port };
    // A path or query has no business on a cache URL; the doctor takes
    // the authority alone.
    let authority = authority.split(['/', '?']).next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.to_string(), p.parse().ok()?)
        }
        _ => (authority.to_string(), default_port),
    };
    if host.is_empty() {
        return None;
    }
    Some(RemoteEndpoint {
        label: label.to_string(),
        host,
        port,
    })
}

/// The remote endpoints the configured cache handler names (A10): the
/// `redis`/`http` spellings parse to host:port pairs; the sharded
/// composite folds its remote members in declaration order (a local tier
/// is not a shard endpoint — the F69 skip contract); the `s3` spelling
/// carries a bucket, not an endpoint, so nothing is probeable from it.
pub(crate) fn remote_cache_endpoints(handler: &CacheHandlerConfig) -> Vec<RemoteEndpoint> {
    match handler {
        CacheHandlerConfig::Memory | CacheHandlerConfig::Disk | CacheHandlerConfig::S3 { .. } => {
            Vec::new()
        }
        CacheHandlerConfig::Redis { url } => {
            endpoint_from_url("redis", url, 6379).into_iter().collect()
        }
        CacheHandlerConfig::Http { url } => {
            endpoint_from_url("http", url, 80).into_iter().collect()
        }
        CacheHandlerConfig::Sharded { shards } => {
            shards.iter().flat_map(remote_cache_endpoints).collect()
        }
    }
}

/// §V2-E08 item 3, check 1 (A10): the configured cache handler's remote
/// reachability. One row per remote member: `Ok` when the probe connects,
/// `Warn` otherwise — the §11.2 contract makes a remote cache an
/// OPTIMIZATION (the F60 posture degrades a transport failure to a loud
/// miss), so a down plane is a lost optimization, never an outage. An
/// `s3` member reports informationally (a bucket name probes as nothing).
pub(crate) fn cache_handler_reachability(
    handler: &CacheHandlerConfig,
    probes: &DoctorProbes,
) -> Vec<CheckRow> {
    let mut rows = Vec::new();
    // A declaration-order walk: the sharded composite folds its members
    // in the order the config declared them.
    fn walk(handler: &CacheHandlerConfig, probes: &DoctorProbes, rows: &mut Vec<CheckRow>) {
        match handler {
            CacheHandlerConfig::Memory | CacheHandlerConfig::Disk => {}
            CacheHandlerConfig::Sharded { shards } => {
                for shard in shards {
                    walk(shard, probes, rows);
                }
            }
            CacheHandlerConfig::S3 { bucket } => {
                rows.push(CheckRow {
                    what: format!(
                        "s3 cache handler: bucket `{bucket}` — reachability not probeable from a bucket name alone (the handler resolves its own endpoint)"
                    ),
                    verdict: Verdict::Skip,
                });
            }
            CacheHandlerConfig::Redis { url } | CacheHandlerConfig::Http { url } => {
                let (label, default_port) = match handler {
                    CacheHandlerConfig::Redis { .. } => ("redis", 6379),
                    _ => ("http", 80),
                };
                match endpoint_from_url(label, url, default_port) {
                    Some(ep) => {
                        let target = format!("{}:{}", ep.host, ep.port);
                        if (probes.tcp_connect)(&ep.host, ep.port) {
                            rows.push(CheckRow {
                                what: format!("{label} cache handler reachable ({target})"),
                                verdict: Verdict::Ok,
                            });
                        } else {
                            rows.push(CheckRow {
                                what: format!(
                                    "{label} cache handler unreachable ({target}) — every entry answers a loud miss (the §11.2 degrade), the plane itself is down"
                                ),
                                verdict: Verdict::Warn,
                            });
                        }
                    }
                    None => {
                        rows.push(CheckRow {
                            what: format!("{label} cache handler: `{url}` parses to no probeable host — check the spelling"),
                            verdict: Verdict::Warn,
                        });
                    }
                }
            }
        }
    }
    walk(handler, probes, &mut rows);
    rows
}

/// §V2-E08 item 3, check 2: the server port's availability — a taken port
/// refuses the dev/start boot outright, so a bind the probe cannot take
/// is a doctor FAILURE, not a warning.
pub(crate) fn port_availability(host: &str, port: u16, probes: &DoctorProbes) -> CheckRow {
    if (probes.try_bind)(host, port) {
        CheckRow {
            what: format!("port {port} on {host} is bindable (the dev/start server can boot)"),
            verdict: Verdict::Ok,
        }
    } else {
        CheckRow {
            what: format!(
                "port {port} on {host} is taken — dev/start refuses to boot until it frees (the listener binds {host})"
            ),
            verdict: Verdict::Fail,
        }
    }
}

/// §V2-E08 item 3, check 3: the open-files soft limit — under the
/// spec's 4,096 the doctor WARNS (a real app exhausts the default 1024;
/// the limit never fails the doctor by itself).
pub(crate) fn fd_limit_check(limit: Option<u64>) -> CheckRow {
    match limit {
        None => CheckRow {
            what: "file-descriptor limit: not exposed on this platform (skipped)".to_string(),
            verdict: Verdict::Skip,
        },
        Some(n) if n < FD_LIMIT_WARN_UNDER => CheckRow {
            what: format!(
                "file-descriptor limit: {n} < {FD_LIMIT_WARN_UNDER} — a dev server holding sockets, watch roots and asset handles hits EMFILE; raise with `ulimit -n`"
            ),
            verdict: Verdict::Warn,
        },
        Some(n) => CheckRow {
            what: format!("file-descriptor limit: {n} (≥ {FD_LIMIT_WARN_UNDER})"),
            verdict: Verdict::Ok,
        },
    }
}

/// §V2-E08 item 3, check 4: the signed-cookie clock skew — the AEAD
/// cookie timestamps judge against the server's own clock, so a drifted
/// clock misjudges every token window. The probe pairs the local clock
/// with an HTTP time authority's `Date` header (the configured cache
/// plane, when one speaks HTTP); `None` — no authority reachable — skips.
pub(crate) fn clock_skew_check(pair: Option<(i64, i64)>) -> CheckRow {
    match pair {
        None => CheckRow {
            what: "clock skew: no HTTP time authority reachable (skipped)".to_string(),
            verdict: Verdict::Skip,
        },
        Some((system, authority)) => {
            let skew = (system - authority).abs();
            if skew > CLOCK_SKEW_TOLERANCE_SECS {
                CheckRow {
                    what: format!(
                        "clock skew {skew}s > {CLOCK_SKEW_TOLERANCE_SECS}s against the cache plane's Date header — signed-cookie windows and build-id expiry misjudge; sync the host clock"
                    ),
                    verdict: Verdict::Fail,
                }
            } else {
                CheckRow {
                    what: format!(
                        "clock skew {skew}s ≤ {CLOCK_SKEW_TOLERANCE_SECS}s against the cache plane's Date header"
                    ),
                    verdict: Verdict::Ok,
                }
            }
        }
    }
}

// ------------------------------------------------------------- real probes

/// The real reachability probe: a 1-second-bounded TCP connect. DNS
/// resolution rides the same bound's to_socket_addrs call (a doctor check
/// may wait one second; it may not hang).
pub(crate) fn tcp_connect(host: &str, port: u16) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .find(|addr| TcpStream::connect_timeout(addr, Duration::from_secs(1)).is_ok())
        .is_some()
}

/// The real bind probe: take the listener and drop it (the dev/start
/// server re-binds at boot).
pub(crate) fn try_bind(host: &str, port: u16) -> bool {
    std::net::TcpListener::bind((host, port)).is_ok()
}

/// The real fd-limit probe: Linux exposes the soft limit in
/// `/proc/self/limits` (`Max open files`); a platform without the file
/// answers `None` (the check skips).
pub(crate) fn fd_limit() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/limits").ok()?;
    let line = text.lines().find(|l| l.starts_with("Max open files"))?;
    // `Max open files <soft> <hard> <units>` — the soft limit is the
    // first number after the fixed prefix.
    let rest = line.strip_prefix("Max open files")?.trim_start();
    let soft = rest.split_whitespace().next()?;
    soft.parse().ok()
}

/// The real time-authority probe: an HTTP/1.0 HEAD to the first PLAIN-HTTP
/// cache-plane endpoint (the `https` spelling needs TLS the std socket
/// cannot speak — with only an `https` plane configured the check skips
/// honestly, never a fabricated verdict), the `Date` header paired with
/// the local clock. Every failure arm answers `None` — no authority
/// reachable means nothing probed.
pub(crate) fn authority_clock(handler: &CacheHandlerConfig) -> Option<(i64, i64)> {
    use std::io::Read;
    use std::net::ToSocketAddrs;
    let ep = remote_cache_endpoints(handler)
        .into_iter()
        .find(|ep| ep.label == "http" && ep.port != 443)?;
    let mut stream = std::net::TcpStream::connect_timeout(
        &format!("{}:{}", ep.host, ep.port)
            .to_socket_addrs()
            .ok()?
            .next()?,
        std::time::Duration::from_secs(1),
    )
    .ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .ok()?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(1)))
        .ok()?;
    // `Connection: close` — one request, one answer, the socket dies with
    // the check (the doctor probes, it never pools).
    let _ = std::io::Write::write_all(
        &mut stream,
        format!(
            "HEAD / HTTP/1.0\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            ep.host, ep.port
        )
        .as_bytes(),
    );
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let date = text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("date").then(|| v.trim().to_string())
    })?;
    let authority = chrono::DateTime::parse_from_rfc2822(&date)
        .ok()?
        .timestamp();
    let system = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((system, authority))
}

#[cfg(test)]
mod pins {
    //! The §V2-E08 item-3 pins: table-driven over fake probes — no pin
    //! touches the network, the filesystem or the wall clock.

    use super::*;
    use oxide_fw_config::CacheHandlerConfig;

    /// The down-probe set: both network probes refuse — the base fixture
    /// the pins decorate.
    fn down_probes() -> DoctorProbes {
        DoctorProbes {
            tcp_connect: |_host: &str, _port: u16| false,
            try_bind: |_host: &str, _port: u16| false,
        }
    }

    // ----------------------------------------------------------- A10 reach

    #[test]
    fn local_tiers_name_no_remote_endpoint() {
        assert!(remote_cache_endpoints(&CacheHandlerConfig::Memory).is_empty());
        assert!(remote_cache_endpoints(&CacheHandlerConfig::Disk).is_empty());
    }

    #[test]
    fn redis_and_http_urls_parse_to_endpoints() {
        let eps = remote_cache_endpoints(&CacheHandlerConfig::Redis {
            url: "redis://cache.internal:6380".to_string(),
        });
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].host, "cache.internal");
        assert_eq!(eps[0].port, 6380);
        assert!(eps[0].label.contains("redis"), "{}", eps[0].label);

        let eps = remote_cache_endpoints(&CacheHandlerConfig::Http {
            url: "https://plane.example.com".to_string(),
        });
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].host, "plane.example.com");
        assert_eq!(eps[0].port, 443, "https defaults to 443");
        assert!(eps[0].label.contains("http"), "{}", eps[0].label);
    }

    #[test]
    fn sharded_handler_folds_its_remote_members_and_skips_local_tiers() {
        let eps = remote_cache_endpoints(&CacheHandlerConfig::Sharded {
            shards: vec![
                CacheHandlerConfig::Memory,
                CacheHandlerConfig::Redis {
                    url: "redis://shard-a:6379".to_string(),
                },
            ],
        });
        assert_eq!(eps.len(), 1, "memory is a local tier, not a shard endpoint");
        assert_eq!(eps[0].host, "shard-a");
    }

    #[test]
    fn s3_names_no_probeable_endpoint() {
        let eps = remote_cache_endpoints(&CacheHandlerConfig::S3 {
            bucket: "oxide-cache".to_string(),
        });
        assert!(
            eps.is_empty(),
            "no host:port is derivable from a bucket name alone"
        );
    }

    #[test]
    fn reachable_remote_answers_ok_and_unreachable_warns() {
        let handler = CacheHandlerConfig::Redis {
            url: "redis://cache.internal:6379".to_string(),
        };
        let up = DoctorProbes {
            tcp_connect: |_host: &str, _port: u16| true,
            ..down_probes()
        };
        let rows = cache_handler_reachability(&handler, &up);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].verdict, Verdict::Ok, "{}", rows[0].what);

        let rows = cache_handler_reachability(&handler, &down_probes());
        assert_eq!(
            rows[0].verdict,
            Verdict::Warn,
            "the §11.2 degrade contract: a down remote cache is an optimization lost, never an outage — {}",
            rows[0].what
        );
        assert!(
            rows[0].what.contains("cache.internal:6379"),
            "{}",
            rows[0].what
        );
    }

    #[test]
    fn s3_bucket_reports_informationally_without_a_probe() {
        let rows = cache_handler_reachability(
            &CacheHandlerConfig::S3 {
                bucket: "oxide-cache".to_string(),
            },
            &down_probes(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].verdict, Verdict::Skip, "{}", rows[0].what);
        assert!(rows[0].what.contains("oxide-cache"), "{}", rows[0].what);
    }

    // --------------------------------------------------------------- port

    #[test]
    fn free_port_answers_ok_and_a_taken_port_fails() {
        let free = DoctorProbes {
            try_bind: |_host: &str, _port: u16| true,
            ..down_probes()
        };
        let row = port_availability("127.0.0.1", 3000, &free);
        assert_eq!(row.verdict, Verdict::Ok, "{}", row.what);

        let row = port_availability("127.0.0.1", 3000, &down_probes());
        assert_eq!(
            row.verdict,
            Verdict::Fail,
            "a taken port refuses the dev/start boot — a doctor failure — {}",
            row.what
        );
        assert!(row.what.contains("3000"), "{}", row.what);
    }

    // ----------------------------------------------------------------- fd

    #[test]
    fn fd_limit_warns_under_4096() {
        let row = fd_limit_check(Some(1024));
        assert_eq!(row.verdict, Verdict::Warn, "{}", row.what);
        assert!(row.what.contains("1024"), "{}", row.what);

        let row = fd_limit_check(Some(65536));
        assert_eq!(row.verdict, Verdict::Ok, "{}", row.what);
    }

    #[test]
    fn fd_limit_skips_when_the_platform_exposes_none() {
        let row = fd_limit_check(None);
        assert_eq!(row.verdict, Verdict::Skip, "{}", row.what);
    }

    // --------------------------------------------------------------- skew

    #[test]
    fn clock_skew_fails_beyond_the_tolerance() {
        let row = clock_skew_check(Some((1_000_000, 1_000_120)));
        assert_eq!(row.verdict, Verdict::Fail, "{}", row.what);
        assert!(row.what.contains("120"), "{}", row.what);
    }

    #[test]
    fn clock_skew_within_the_tolerance_answers_ok() {
        let row = clock_skew_check(Some((1_000_000, 1_000_030)));
        assert_eq!(row.verdict, Verdict::Ok, "{}", row.what);
    }

    #[test]
    fn clock_skew_skips_without_a_time_authority() {
        let row = clock_skew_check(None);
        assert_eq!(row.verdict, Verdict::Skip, "{}", row.what);
    }
}
