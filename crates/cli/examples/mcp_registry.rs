//! Consumer example for the V2-E07 seam: the MCP tool registry and the
//! `llms.txt` emission.
//!
//! Shows the pieces the feature composes:
//!
//! 1. [`oxide_fw_cli::mcp::default_registry`] — every tool the server
//!    serves, each with a name, description, JSON Schema input and a
//!    `writes()` flag. `tools/list` renders exactly this.
//! 2. [`oxide_fw_cli::mcp::OxideMcp::dispatch`] — the registry dispatch:
//!    a read tool runs, an unknown name is `METHOD_NOT_FOUND`, and a write
//!    tool (`build.run`) is refused with a named refusal unless the server
//!    runs with `--allow-write`.
//! 3. [`oxide_fw_cli::mcp::redact_secrets`] — the config.get redaction:
//!    every secret-shaped key (the request log's `is_secret_key` rule)
//!    never leaves the process.
//! 4. [`oxide_fw_cli::llms::llms_txt`] — the `dist/llms.txt` shape the
//!    build emits when `llms.enabled` (title, summary, included routes
//!    with their meta descriptions).
//!
//! Run it:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example mcp_registry
//! ```

use oxide_fw_cli::llms::llms_txt;
use oxide_fw_cli::mcp::{McpCx, OxideMcp, default_registry, redact_secrets};
use serde_json::json;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    // 1. The registry: the stable tools/list order.
    println!("1. the registry (tools/list order):");
    for t in default_registry() {
        println!(
            "   {:<14} writes: {:<5} — {}",
            t.name(),
            t.writes(),
            t.description()
        );
    }

    // 2. Dispatch: a read tool against a scratch project (empty cache, no
    //    build — the honest empty reports), then the write refusal.
    let scratch = std::env::temp_dir().join("oxide-mcp-registry-example");
    std::fs::create_dir_all(&scratch).unwrap();
    let server = OxideMcp::new(scratch.clone());
    let cache = server
        .dispatch("cache.inspect", json!({}))
        .await
        .expect("read tool runs");
    println!("\n2. cache.inspect -> {}", first_text(&cache));
    let refusal = server
        .dispatch("build.run", json!({}))
        .await
        .expect("the refusal is an Ok(error result)");
    println!(
        "   build.run (read-only server) -> {}",
        first_text(&refusal)
    );

    // 3. The redaction: secret-shaped keys never leave the process.
    let mut config = json!({
        "adapter": "binary",
        "environments": { "production": { "apiKey": "s3cret", "port": 3000 } }
    });
    let n = redact_secrets(&mut config);
    println!("\n3. config.get redaction: {n} key(s) redacted -> {config}");

    // 4. The llms.txt shape the build emits when `llms.enabled`.
    let txt = llms_txt(
        "Oxide",
        Some("The framework-agnostic meta-framework"),
        &[
            ("/".to_string(), "Home".to_string()),
            ("/docs/intro".to_string(), "The intro guide".to_string()),
        ],
    );
    println!("\n4. dist/llms.txt when llms.enabled:\n{txt}");

    // The dispatch context a real server runs with:
    let _ = McpCx {
        root: PathBuf::from("."),
        allow_write: false,
    };
}

fn first_text(r: &rmcp::model::CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}
