//! The Appendix V2.J endpoint map — the one inventory of the HTTP
//! surfaces the framework serves, as data, so the docs generator folds it
//! (the capability-matrix rule: the map is the single source of truth;
//! `docs/reference/endpoints.md` is a consumer, never a second opinion).
//!
//! The plane rule is structural, not narrative: `/__oxide/*` is the data
//! plane (build-header checked, A04), `/_oxide/*` is the ops plane, and
//! the dev-only surfaces are dev-token guarded (D41) — the invariant the
//! [`ENDPOINT_MAP`] rows carry and the tests hold row by row.

/// The plane an endpoint serves (§V2.J: data, ops, dev).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointPlane {
    /// `/__oxide/*` (and the loader data surface) — the data plane, the
    /// build-header check guards it (A04).
    Data,
    /// `/_oxide/*` — the ops plane.
    Ops,
    /// Dev-only surfaces, dev-token guarded (D41).
    Dev,
}

impl EndpointPlane {
    /// The cell text the generated table renders (the spec's column).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Ops => "ops",
            Self::Dev => "dev",
        }
    }
}

/// One endpoint-map row: the URL shape (with its query spelling), the
/// method, the plane, the work packages that landed or own it, and the
/// one-line purpose.
#[derive(Debug, Clone, Copy)]
pub struct EndpointRow {
    /// The path (or path family, braces spelling the group) as the spec
    /// names it.
    pub path: &'static str,
    /// The HTTP method (or `various` for the grouped ops surfaces).
    pub method: &'static str,
    /// The plane the row serves.
    pub plane: EndpointPlane,
    /// The work packages: `existing` plus the V2 WPs that landed here.
    pub work_packages: &'static str,
    /// The one-line purpose.
    pub purpose: &'static str,
}

/// The §V2.J endpoint map, verbatim. New HTTP surfaces join here (the
/// docs-check gate then fails until the committed page regenerates).
pub const ENDPOINT_MAP: &[EndpointRow] = &[
    EndpointRow {
        path: "/__data?route=",
        method: "GET",
        plane: EndpointPlane::Data,
        work_packages: "existing, A04, A05",
        purpose: "Route loader data as ODF; now gated by middleware and carries `x-oxide-build`",
    },
    EndpointRow {
        path: "/__oxide/live",
        method: "GET (SSE)",
        plane: EndpointPlane::Data,
        work_packages: "F05",
        purpose: "Live loader snapshots and patches",
    },
    EndpointRow {
        path: "/__oxide/stream/<id>",
        method: "GET",
        plane: EndpointPlane::Data,
        work_packages: "F07",
        purpose: "Reattach to a resumable stream with `Range`",
    },
    EndpointRow {
        path: "/__oxide/state",
        method: "GET",
        plane: EndpointPlane::Data,
        work_packages: "existing",
        purpose: "Hydration state for tooling",
    },
    EndpointRow {
        path: "/_oxide/fn/<id>",
        method: "POST",
        plane: EndpointPlane::Ops,
        work_packages: "existing, F08",
        purpose: "Server functions (TypeScript and Rust)",
    },
    EndpointRow {
        path: "/_oxide/search",
        method: "GET",
        plane: EndpointPlane::Ops,
        work_packages: "F09",
        purpose: "Full-text query",
    },
    EndpointRow {
        path: "/_oxide/nav?from=",
        method: "GET",
        plane: EndpointPlane::Ops,
        work_packages: "F13",
        purpose: "Aggregate next-route probabilities",
    },
    EndpointRow {
        path: "/_oxide/csp-report",
        method: "POST",
        plane: EndpointPlane::Ops,
        work_packages: "E04",
        purpose: "CSP violation reports",
    },
    EndpointRow {
        path: "/_oxide/health, /_oxide/ready",
        method: "GET",
        plane: EndpointPlane::Ops,
        work_packages: "existing, F12",
        purpose: "Liveness and readiness (level reported)",
    },
    EndpointRow {
        path: "/_oxide/{vitals,static,og,metrics,image,hmr}",
        method: "various",
        plane: EndpointPlane::Ops,
        work_packages: "existing",
        purpose: "Unchanged",
    },
    EndpointRow {
        path: "/__oxide/open, /__oxide/cost",
        method: "GET",
        plane: EndpointPlane::Dev,
        work_packages: "E08, F17",
        purpose: "Open in editor; component cost (dev-token guarded)",
    },
    EndpointRow {
        path: "/__oxide/traces",
        method: "GET",
        plane: EndpointPlane::Dev,
        work_packages: "F21, F23",
        purpose: "Recent-request trace feed over the §20 context; the window answers filtered (?status=, ?method=, ?path=, ?limit=; dev-token guarded)",
    },
    EndpointRow {
        path: "/__oxide/logs",
        method: "GET",
        plane: EndpointPlane::Dev,
        work_packages: "F24",
        purpose: "Recent log-event feed over the capture layer; the window answers filtered (?level=, ?target=, ?q=, ?limit=; dev-token guarded)",
    },
    EndpointRow {
        path: "/__oxide",
        method: "GET",
        plane: EndpointPlane::Dev,
        work_packages: "existing, E08",
        purpose: "Inspector",
    },
];

/// The GFM table the docs generator folds: one row per [`ENDPOINT_MAP`]
/// entry, deterministic bytes on every call (the property the
/// `--check` gate holds).
#[must_use]
pub fn endpoint_markdown_table() -> String {
    use std::fmt::Write as _;
    let mut out = String::from("| path | method | plane | WP | purpose |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for row in ENDPOINT_MAP {
        // Deterministic bytes; `writeln!` appends without the intermediate
        // `format!` allocation the `push_str(&format!(…))` shape pays.
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} |",
            row.path.replace('|', "\\|"),
            row.method.replace('|', "\\|"),
            row.plane.as_str(),
            row.work_packages.replace('|', "\\|"),
            row.purpose.replace('|', "\\|")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row lands on its plane's URL family: data and dev ride the
    /// `/__` prefix (the build-header check and the dev-token guard), ops
    /// rides `/_oxide`. The V2.J plane rule, held row by row.
    #[test]
    fn every_row_lands_on_its_plane_family() {
        for row in ENDPOINT_MAP {
            match row.plane {
                EndpointPlane::Data => assert!(
                    row.path.starts_with("/__data") || row.path.starts_with("/__oxide"),
                    "{} is not a data-plane path",
                    row.path
                ),
                EndpointPlane::Ops => assert!(
                    row.path.starts_with("/_oxide"),
                    "{} is not an ops-plane path",
                    row.path
                ),
                EndpointPlane::Dev => assert!(
                    row.path.starts_with("/__oxide"),
                    "{} is not a dev-plane path",
                    row.path
                ),
            }
        }
    }

    /// Every row carries a path, a method and a purpose — the table has
    /// no blank cells for the docs-check gate to render.
    #[test]
    fn every_row_is_complete() {
        assert_eq!(ENDPOINT_MAP.len(), 14, "the spec's map has 14 rows");
        for row in ENDPOINT_MAP {
            assert!(!row.path.is_empty());
            assert!(!row.method.is_empty());
            assert!(!row.work_packages.is_empty());
            assert!(!row.purpose.is_empty());
        }
    }

    /// The table renders deterministically (identical bytes on every
    /// call — the property `--check` holds), carries every path, and the
    /// GFM pipes stay escaped.
    #[test]
    fn table_renders_deterministically() {
        let first = endpoint_markdown_table();
        assert_eq!(first, endpoint_markdown_table());
        for row in ENDPOINT_MAP {
            assert!(
                first.contains(&format!("| `{}` |", row.path)),
                "{}",
                row.path
            );
        }
        assert!(first.starts_with("| path | method | plane | WP | purpose |\n"));
        assert_eq!(first.lines().count(), ENDPOINT_MAP.len() + 2);
    }

    /// The landed work packages the map names stay honest: the search
    /// endpoint names F09, the resumable stream F07, the fn surface F08.
    #[test]
    fn work_packages_name_the_landing_rows() {
        let by_path = |p: &str| {
            ENDPOINT_MAP
                .iter()
                .find(|r| r.path == p)
                .unwrap_or_else(|| panic!("{p} missing"))
        };
        assert_eq!(by_path("/_oxide/search").work_packages, "F09");
        assert_eq!(by_path("/__oxide/stream/<id>").work_packages, "F07");
        assert_eq!(by_path("/_oxide/fn/<id>").work_packages, "existing, F08");
    }

    /// The regen path for the committed page's table block (the E11
    /// pattern): `cargo test -p oxide-fw-server --lib endpoints -- --ignored
    /// --nocapture` prints the exact bytes the docs generator folds.
    #[test]
    #[ignore]
    fn regen_prints_the_table_bytes() {
        print!("\n{}", endpoint_markdown_table());
    }
}
