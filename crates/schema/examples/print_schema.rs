//! Consumer walkthrough for the schema crate: what `oxide schema`
//! prints and what the `$schema` URL points at.
//!
//! The schema is the contract between a user's `oxide.config.ts` and
//! their editor: the `$schema` field points at the canonical URL
//! ([`SCHEMA_URL`]) and the editor validates the config against the
//! generated JSON Schema in real time. The crate delegates to the config
//! crate's generator (`config_schema_pretty`) so there is exactly one
//! source of truth for the shape; this example walks the three public
//! surfaces a consumer rides — the URL, the pretty-printed document and
//! its re-parsed [`serde_json::Value`] form (the `SCHEMA-002` class the
//! crate guards).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-schema --example print_schema
//! ```

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "the canonical $schema URL: {}",
        oxide_fw_schema::schema_url()
    );

    let pretty = oxide_fw_schema::schema_pretty()?;
    println!(
        "the schema document: {} bytes, top-level type: object",
        pretty.len()
    );

    // The typed half: the pretty document re-parses (SCHEMA-002 guards
    // the impossible-not-JSON arm — a serde bug class, not user input).
    let value = oxide_fw_schema::schema_value()?;
    let properties = value
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("the schema declares top-level properties");
    println!("top-level config sections ({}): {}", properties.len(), {
        let mut names: Vec<&String> = properties.keys().collect();
        names.sort();
        names
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    });
    Ok(())
}
