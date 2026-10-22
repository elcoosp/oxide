//! `oxide-schema` — writes the JSON Schema for `oxide.config.{ts,json}` to
//! stdout (spec §19.3: "JSON Schema for config").
//!
//! Editors and IDEs point `$schema` at the checked-in artifact
//! (`schemas/oxide-config.schema.json`); this binary is the regenerator.
//! A drift gate in `crates/config/tests/schema_stability.rs` fails CI if
//! the checked-in file and the function's output diverge — so adding a
//! config key without regenerating is a build error, not a silent staleness.
//!
//! Usage:
//!   cargo run -p oxide-config --bin oxide-schema -- > schemas/oxide-config.schema.json

fn main() -> std::process::ExitCode {
    match oxide_config::config_schema_pretty() {
        Ok(s) => {
            print!("{s}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oxide-schema: {} {}", e.0.code, e.0.message);
            std::process::ExitCode::FAILURE
        }
    }
}
