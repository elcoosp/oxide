//! TDD pins for the §V2-F01 mixed-framework islands config surface —
//! the `islands` union (`string | { src, framework? }`) and the
//! inference order the spec pins (explicit `framework`; file extension
//! — `.vue` is vue, `.svelte` is svelte; a first-line pragma
//! `// @oxide-framework solid` for `.tsx`; else the project
//! `framework`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The compiler-side boot planner that consumes the resolved entries is
//! `oxide_fw_compiler::islands` (the same milestone); the CLI reads the
//! files and applies [`island_framework`] — the pure rule lives here.

use oxide_fw_config::{FrameworkConfig, IslandSpec, island_framework};

// ------------------------------------------------------------ inference

/// The spec's inference order, one arm per case (8 cases).
#[test]
fn island_framework_inference_table() {
    // (src, explicit, project, first_line) → expected
    let cases: [(&str, Option<&str>, &str, &str, &str); 8] = [
        // 1. an explicit `framework` wins over everything.
        (
            "src/c.tsx",
            Some("solid"),
            "react",
            "// @oxide-framework vue",
            "solid",
        ),
        // 2. the `.vue` extension infers before any pragma.
        ("src/Price.vue", None, "react", "", "vue"),
        // 3. the `.svelte` extension infers before any pragma.
        ("src/Ticker.svelte", None, "solid", "", "svelte"),
        // 4. a first-line pragma infers a `.tsx` island.
        (
            "src/Ticker.tsx",
            None,
            "react",
            "// @oxide-framework solid",
            "solid",
        ),
        // 5. the pragma tolerates surrounding whitespace.
        (
            "src/Ticker.tsx",
            None,
            "react",
            "  // @oxide-framework solid  ",
            "solid",
        ),
        // 6. no arm matches: the project framework.
        (
            "src/counter.tsx",
            None,
            "react",
            "import x from 'y'",
            "react",
        ),
        // 7. a non-vue/svelte extension does not infer.
        ("src/counter.ts", None, "vue", "", "vue"),
        // 8. the spec's needle carries a trailing space, so a bare
        //    `// @oxide-framework` line is not a pragma: the project
        //    framework answers.
        ("src/c.tsx", None, "react", "// @oxide-framework", "react"),
    ];
    for (src, explicit, project, first_line, expected) in cases {
        assert_eq!(
            island_framework(src, explicit, project, first_line),
            expected,
            "case src={src} explicit={explicit:?} first_line={first_line:?}"
        );
    }
}

#[test]
fn island_framework_prefers_explicit_over_extension_and_pragma() {
    // The explicit arm is FIRST: a `.vue` file declared solid stays solid.
    assert_eq!(
        island_framework("src/a.vue", Some("solid"), "react", ""),
        "solid"
    );
}

// ---------------------------------------------------------- the union

#[test]
fn islands_accept_the_plain_string_form() {
    let cfg: FrameworkConfig =
        serde_json::from_str(r#"{ "islands": { "Counter": "src/islands/counter.tsx" } }"#)
            .expect("the string form parses");
    let spec = cfg.islands.get("Counter").expect("the entry");
    assert_eq!(spec.src(), "src/islands/counter.tsx");
    assert_eq!(spec.framework(), None);
}

#[test]
fn islands_accept_the_object_form() {
    let cfg: FrameworkConfig = serde_json::from_str(
        r#"{ "islands": { "Price": { "src": "src/islands/Price.vue" },
                            "Ticker": { "src": "src/islands/ticker.tsx", "framework": "solid" } } }"#,
    )
    .expect("the object form parses");
    let price = cfg.islands.get("Price").expect("the entry");
    assert_eq!(price.src(), "src/islands/Price.vue");
    assert_eq!(price.framework(), None);
    let ticker = cfg.islands.get("Ticker").expect("the entry");
    assert_eq!(ticker.src(), "src/islands/ticker.tsx");
    assert_eq!(ticker.framework(), Some("solid"));
}

#[test]
fn islands_union_serializes_back_to_its_input_shape() {
    // The plain form serializes as the bare string (the config file's
    // shape is stable across a load/save round trip).
    let plain = IslandSpec::Plain("src/a.tsx".into());
    assert_eq!(serde_json::to_string(&plain).unwrap(), r#""src/a.tsx""#);
    let object = IslandSpec::Object {
        src: "src/b.tsx".into(),
        framework: Some("solid".into()),
    };
    assert_eq!(
        serde_json::to_string(&object).unwrap(),
        r#"{"src":"src/b.tsx","framework":"solid"}"#
    );
}

// ------------------------------------------------------------- schema

#[test]
fn the_schema_describes_the_islands_union() {
    let schema = oxide_fw_config::config_schema();
    let islands = schema
        .pointer("/properties/framework/properties/islands")
        .expect("the islands entry is in the schema");
    let json = serde_json::to_string(islands).unwrap();
    assert!(json.contains("src"), "the object form's src key: {json}");
    assert!(
        json.contains("framework"),
        "the object form's framework key: {json}"
    );
}
