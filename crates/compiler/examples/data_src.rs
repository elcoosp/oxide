//! The §V2-F17 `data-ox-src` walkthrough: the DOM→source mapping the dev
//! transform stamps onto every top-level component's first rendered
//! element. A route module with a page component and a bound arrow is
//! injected, the stamped source is printed, and the stamp is shown
//! riding the dev lowering's output (the props call the JSX runtime
//! receives) — the value the overlay reads to attribute a hovered DOM
//! node to `path:line` and to fetch that component's `/__oxide/cost`
//! record without re-spelling the path.
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example data_src
//! ```

use oxide_fw_compiler::{inject_data_ox_src, lower_to_js_with_refresh};

fn main() {
    // A route module: the default-export page and a const-bound arrow —
    // the two shapes the rule recognizes (the helper fn with a lowercase
    // name stays unstamped).
    let module = "import Card from './Card';\n\nexport default function Page() {\n  return (\n    <main>\n      <Card title=\"hello\" />\n    </main>\n  );\n}\n\nconst Footer = () => (\n  <footer className=\"f\">fin</footer>\n);\n\nfunction makeRow(label: string) {\n  return <li>{label}</li>;\n}\n";
    println!("== the module (unstamped) ==");
    println!("{module}");

    // The stamp: span surgery on the original text — only the attribute
    // bytes are inserted, so every line count (and the dev sourcemap)
    // survives.
    let stamped = inject_data_ox_src(module, "tsx", "src/routes/page.tsx")
        .unwrap()
        .expect("the module has two components to stamp");
    println!("== the stamped module (dev serve) ==");
    println!("{stamped}");

    // The idempotency contract: re-injecting the stamped source is a
    // no-op (the dev pipeline re-serves modules).
    let re = inject_data_ox_src(&stamped, "tsx", "src/routes/page.tsx").unwrap();
    println!(
        "re-injection: {}",
        match re {
            None => "unchanged (the elements are already attributed)".to_string(),
            Some(_) => "UNEXPECTED second stamp".to_string(),
        }
    );

    // The dev lowering composition: the stamp rides the JSX lowering into
    // the props call, and the refresh preamble lands after it — both dev
    // passes compose through `lower_to_js_with_refresh`, the entry the
    // ESM dev server serves modules through.
    let (lowered, _) =
        lower_to_js_with_refresh(&stamped, "tsx", "src/routes/page.tsx", None).unwrap();
    println!("== the dev lowering carries the stamp ==");
    for line in lowered
        .lines()
        .filter(|l| l.contains("data-ox-src") || l.contains("$RefreshReg$("))
    {
        println!("  {line}");
    }

    // The no-vouch direction: the lowercase helper is not a component.
    println!(
        "\nthe helper `makeRow` carries no stamp (lowercase names are not components) — the overlay only attributes what components render"
    );
}
