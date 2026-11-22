//! V2-F19: the `declares_permissions` scanner — the per-route
//! capability-permissions export scan. Mirrors `declares_search_schema`
//! exactly (the token-boundary honest shape): line-based,
//! `strip_prefix("export const permissions")` + a boundary check so a
//! longer name (`permissionsV2`, `permissionsPolicy`) or a `function`
//! export does not match.
//!
//! Red-first per the house TDD rule: the scanner failed before
//! `declares_permissions` existed (the function was missing). The pins
//! hold it to the token boundary the spec's declaration contract names.

/// A `page.server.ts` source that declares `permissions` — the spec's
/// example shape (`{ net, env, fs }`).
const PERM_SRC: &str = "export const permissions = {\n  net: ['api.stripe.com', '*.internal'],\n  env: ['STRIPE_KEY'],\n  fs: ['./data'],\n};\n";

#[test]
fn declares_permissions_matches_a_const_export() {
    assert!(oxide_fw_router::declares_permissions(PERM_SRC));
    // A typed initializer (`: PermType = …`) matches too.
    assert!(oxide_fw_router::declares_permissions(
        "export const permissions: { net: string[] } = { net: ['x'] };"
    ));
    // Indented (the real `page.server.ts` body indent).
    assert!(oxide_fw_router::declares_permissions(
        "  export const permissions = { net: ['x'] };"
    ));
}

#[test]
fn declares_permissions_rejects_a_function_export() {
    assert!(!oxide_fw_router::declares_permissions(
        "export function permissions() { return { net: [] }; }"
    ));
}

#[test]
fn declares_permissions_rejects_a_longer_binding_name() {
    // `permissionsV2` / `permissionsPolicy` are different bindings — the
    // token boundary rejects them (the next char after `permissions` is
    // an identifier character, not `=` / `:` / EOF).
    assert!(!oxide_fw_router::declares_permissions(
        "export const permissionsV2 = { net: ['x'] };"
    ));
    assert!(!oxide_fw_router::declares_permissions(
        "export const permissionsPolicy = 'deny';"
    ));
}

#[test]
fn declares_permissions_rejects_an_unrelated_export() {
    assert!(!oxide_fw_router::declares_permissions(
        "export const load = () => ({});"
    ));
    assert!(!oxide_fw_router::declares_permissions(
        "export const config = { live: true };"
    ));
}

#[test]
fn declares_permissions_rejects_empty_source() {
    assert!(!oxide_fw_router::declares_permissions(""));
}

#[test]
fn declares_permissions_rejects_a_commented_export() {
    // A commented-out declaration does not match — the `//` comes before
    // `export` after `trim_start`, so `strip_prefix` fails.
    assert!(!oxide_fw_router::declares_permissions(
        "// export const permissions = { net: ['x'] };"
    ));
}
