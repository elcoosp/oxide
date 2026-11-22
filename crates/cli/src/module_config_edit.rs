//! §V2-E06: the `oxide add` config edit — span-based AST surgery on
//! `oxide.config.ts` that adds the import + the `modules: [...]` array
//! element idempotently, preserves comments, and refuses a dynamic config
//! loudly.
//!
//! Design: the existing `scan_import_spans` (in `oxide-fw-compiler`)
//! tokenizes the file byte-accurately (strings, template literals,
//! comments skipped). The edit reuses the SAME span machinery the
//! `esm_dev::rewrite_imports` precedent established — never a regex,
//! never a full re-serialization that would drop the comments a project's
//! config carries. The edit is:
//!
//! 1. Find the existing `import … from '<pkg-or-alias>'` (idempotency
//!    check). If present, skip the import insert.
//! 2. Find the `defineConfig(` call's object literal. If the file does
//!    not call `defineConfig` (a dynamic config: `export default
//!    JSON.parse(...)` / `export default await fetch(...)`), refuse with
//!    `MOD-CONFIG-DYNAMIC`, printing the two lines to paste.
//! 3. Inside the object literal, find the `modules:` key. If present,
//!    append the module name (idempotency: skip if already in the array).
//!    If absent, insert a `modules: ['<name>'],` entry.
//!
//! The result is a string splice at byte offsets the tokenizer produced
//! — comments and unrelated code stay byte-identical.

use oxide_fw_compiler::scan_import_spans;
use oxide_fw_diagnostics::{Diagnostic, OxideError};

/// The default config alias the spec's `oxide add @oxide/tailwind` expects
/// (`@oxide/tailwind` → `tailwind` locally, so the import reads
/// `import tailwind from '@oxide/tailwind'`). The module's `package.json`
/// `name` is the source of truth; the alias is only a fallback when the
/// caller did not name one.
const DEFAULT_ALIAS: &str = "module";

/// The result of a config edit: the new bytes, whether the import was
/// added, and whether the modules-array element was added. A no-op edit
/// (already present) is a success with both flags false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEdit {
    pub bytes: String,
    pub import_added: bool,
    pub module_added: bool,
}

/// Edit `oxide.config.ts` to add the import + the `modules: [...]` array
/// element for `module`. Idempotent: a second call for the same module is
/// a no-op. Comment-preserving: the edit is a byte splice, not a
/// re-serialization.
///
/// # Errors
///
/// `MOD-CONFIG-DYNAMIC` when the config does not call `defineConfig(`
/// (a dynamic config the editor cannot edit safely). The fix names the
/// two lines to paste. `MOD-CONFIG-PARSE` when the file does not parse
/// as a config (no `defineConfig` and no `export default`).
pub fn add_module_to_config(
    src: &str,
    module: &str,
    alias: Option<&str>,
) -> Result<ConfigEdit, OxideError> {
    let alias = alias.unwrap_or(default_alias_for(module));
    let import_added = !has_import(src, module);
    let module_added = !has_modules_entry(src, module);

    if !import_added && !module_added {
        return Ok(ConfigEdit {
            bytes: src.to_string(),
            import_added: false,
            module_added: false,
        });
    }

    // The defineConfig call site: `defineConfig({ … })` or
    // `defineConfig(async () => ({ … }))`. We only edit the synchronous
    // object-literal shape; a dynamic config (no `defineConfig`, or a
    // `defineConfig(<expr>)` that is not an object literal) is refused.
    let _cfg_start = find_define_config_open(src).ok_or_else(|| {
        OxideError::from(
            Diagnostic::error(
                "MOD-CONFIG-DYNAMIC",
                format!(
                    "oxide.config.ts does not call `defineConfig({{ … }})` with an object literal — the editor cannot safely edit a dynamic config (§V2-E06)"
                ),
            )
            .with_fix(format!(
                "add `modules: ['{module}']` to the `defineConfig({{ … }})` object literal manually, or\n  convert the config to a static `export default defineConfig({{ … }})` shape"
            )),
        )
    })?;

    let mut out = String::with_capacity(src.len() + 128);
    // 1. Insert the import at the top, after any leading comments / shebang.
    let import_insert_at = leading_header_end(src);
    if import_added {
        out.push_str(&src[..import_insert_at]);
        out.push_str("import ");
        out.push_str(alias);
        out.push_str(" from '");
        out.push_str(module);
        out.push_str("'\n");
        out.push_str(&src[import_insert_at..]);
    } else {
        out.push_str(src);
    }

    // 2. Insert/append the modules-array element. Operate on `out` (offsets
    //    shift by the import insert). Re-find the defineConfig open in
    //    the new bytes.
    let cfg_start_new =
        find_define_config_open(&out).expect("defineConfig still present after import insert");
    let cfg_obj_open = find_first_char(&out, cfg_start_new, '{').ok_or_else(|| {
        OxideError::from(Diagnostic::error(
            "MOD-CONFIG-PARSE",
            "oxide.config.ts calls `defineConfig(` but the object literal `{` was not found (§V2-E06)",
        ))
    })?;

    if !module_added {
        return Ok(ConfigEdit {
            bytes: out,
            import_added,
            module_added: false,
        });
    }

    // Find the `modules:` key inside the object literal. If present, append
    // to its array. If absent, insert a new `modules: ['<name>'],` line at
    // the object literal's opening brace.
    let obj_end = find_matching_brace(&out, cfg_obj_open).ok_or_else(|| {
        OxideError::from(Diagnostic::error(
            "MOD-CONFIG-PARSE",
            "oxide.config.ts `defineConfig({ … })` object literal is not balanced (§V2-E06)",
        ))
    })?;
    let obj_body = &out[cfg_obj_open + 1..obj_end];

    if let Some(rel_offset) = find_modules_key(obj_body) {
        // The existing `modules: [...]` array — append the module name.
        let abs_array_start = cfg_obj_open + 1 + rel_offset;
        let abs_array_end = find_first_char(&out, abs_array_start, ']').ok_or_else(|| {
            OxideError::from(Diagnostic::error(
                "MOD-CONFIG-PARSE",
                "oxide.config.ts `modules: […]` array is not closed (§V2-E06)",
            ))
        })?;
        // Insert before the `]`. If the array is non-empty, prepend `, 'name'`.
        let between = &out[abs_array_start..abs_array_end];
        let insert_at = abs_array_end;
        let mut insertion = String::new();
        if !between.trim().is_empty() && !between.trim_end().ends_with(',') {
            insertion.push_str(", ");
        }
        insertion.push_str("'");
        insertion.push_str(module);
        insertion.push_str("'");
        out.insert_str(insert_at, &insertion);
    } else {
        // No `modules:` key — insert a new entry right after the `{`.
        let indent = detect_indent(&out, cfg_obj_open);
        let insertion = format!("\n{indent}modules: ['{module}'],\n");
        out.insert_str(cfg_obj_open + 1, &insertion);
    }

    Ok(ConfigEdit {
        bytes: out,
        import_added,
        module_added: true,
    })
}

/// True when `src` already imports `module` (idempotency check). Uses
/// `scan_import_spans` so a string in a comment does not false-positive.
fn has_import(src: &str, module: &str) -> bool {
    scan_import_spans(src).iter().any(|s| s.specifier == module)
}

/// True when `src`'s `modules:` array already lists `module`.
fn has_modules_entry(src: &str, module: &str) -> bool {
    // A cheap, comment-safe scan: find the `modules:` key, then scan to
    // the closing `]` for a quoted `'<module>'`. The tokenizer does not
    // surface key/value pairs, so we walk the bytes between the key and
    // the array close. Strings inside that range are real (comments are
    // skipped by scanning only outside of `'` / `"` / `` ` `` / `//` /
    // `/*`); a `module` substring inside a string that is not the module
    // name does not match because we require the quote-delimited form.
    let Some(rel) = find_modules_key(src) else {
        return false;
    };
    let abs_start = rel;
    let abs_end = match find_first_char(src, abs_start, ']') {
        Some(e) => e,
        None => return false,
    };
    let needle = format!("'{}'", module);
    let needle2 = format!("\"{}\"", module);
    let body = &src[abs_start..abs_end];
    body.contains(&needle) || body.contains(&needle2)
}

/// Find the byte offset of the `defineConfig(` token's `(`. Returns `None`
/// when the config does not call `defineConfig` (a dynamic config).
fn find_define_config_open(src: &str) -> Option<usize> {
    let bytes = src.as_bytes();
    let needle = b"defineConfig";
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // Skip whitespace to the `(`.
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                return Some(j);
            }
        }
        i += 1;
    }
    None
}

/// Find the first occurrence of `c` at or after `from`, skipping strings
/// and comments. Returns the byte offset.
fn find_first_char(src: &str, from: usize, c: char) -> Option<usize> {
    let bytes = src.as_bytes();
    let cb = c as u8;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_string(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = skip_line_comment(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(bytes, i);
            }
            _ if bytes[i] == cb => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Find the matching `}` for the `{` at `open`, skipping strings/comments.
fn find_matching_brace(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_string(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = skip_line_comment(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(bytes, i);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i - 1);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Skip a string starting at `i` (the quote byte). Returns the offset
/// past the closing quote.
fn skip_string(bytes: &[u8], i: usize) -> usize {
    let quote = bytes[i];
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b if b == quote => return j + 1,
            _ => j += 1,
        }
    }
    j
}

fn skip_line_comment(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 2;
    while j < bytes.len() && bytes[j] != b'\n' {
        j += 1;
    }
    j
}

fn skip_block_comment(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 2;
    while j + 1 < bytes.len() {
        if bytes[j] == b'*' && bytes[j + 1] == b'/' {
            return j + 2;
        }
        j += 1;
    }
    bytes.len()
}

/// Find the relative offset of the `modules:` key inside an object
/// literal body. Returns the offset of the `m` in `modules`.
fn find_modules_key(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let needle = b"modules";
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // The next non-whitespace byte must be `:`.
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// The byte offset where the import block should be inserted — after a
/// leading shebang and any line comments / blank lines at the top of the
/// file. Keeps the project's header comments above the new import.
fn leading_header_end(src: &str) -> usize {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    // Shebang.
    if bytes.starts_with(b"#!") {
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    // Leading `//` / `/* */` / blank lines.
    loop {
        let saved = i;
        // blank lines
        while i < bytes.len() && (bytes[i] == b'\n' || bytes[i] == b'\r' || bytes[i] == b' ') {
            i += 1;
        }
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(bytes, i);
            continue;
        }
        // Not a header line — restore the position before the blank-line
        // skip so the import lands at the start of the first real line.
        let _ = saved;
        break;
    }
    i
}

/// Detect the indentation of the line containing `cfg_obj_open` so the
/// inserted `modules:` entry matches the project's style.
fn detect_indent(src: &str, cfg_obj_open: usize) -> String {
    let bytes = src.as_bytes();
    let mut line_start = cfg_obj_open;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let mut indent = String::new();
    let mut i = line_start;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        indent.push(bytes[i] as char);
        i += 1;
    }
    if indent.is_empty() {
        "  ".to_string()
    } else {
        format!("{indent}  ")
    }
}

/// The default local name for a module import. `@oxide/analytics` →
/// `analytics`; `@scope/pkg` → `pkg`; `pkg` → `pkg`. Falls back to
/// `module` for a name with no parseable identifier. Returns a borrowed
/// slice of `module` (no allocation).
#[must_use]
pub fn default_alias_for(module: &str) -> &str {
    let trimmed = module.strip_prefix('@').unwrap_or(module);
    let last = trimmed.rsplit('/').next().unwrap_or("");
    if last.is_empty() { DEFAULT_ALIAS } else { last }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_STATIC: &str = "// project config\n\
import { defineConfig } from '@oxide-fw/core'\n\
\n\
export default defineConfig({\n\
  framework: { adapter: 'react' },\n\
  // routes live under src/routes\n\
  routes: { dir: 'src/routes' },\n\
})\n";

    #[test]
    fn adds_import_and_modules_entry_idempotently() {
        let edit =
            add_module_to_config(FIXTURE_STATIC, "@oxide/analytics", Some("analytics")).unwrap();
        assert!(edit.import_added);
        assert!(edit.module_added);
        assert!(
            edit.bytes
                .contains("import analytics from '@oxide/analytics'")
        );
        assert!(edit.bytes.contains("modules: ['@oxide/analytics']"));
        // Round-trip: a second call is a no-op.
        let edit2 =
            add_module_to_config(&edit.bytes, "@oxide/analytics", Some("analytics")).unwrap();
        assert!(!edit2.import_added);
        assert!(!edit2.module_added);
        assert_eq!(edit2.bytes, edit.bytes);
    }

    #[test]
    fn preserves_comments_and_unrelated_code() {
        let edit =
            add_module_to_config(FIXTURE_STATIC, "@oxide/analytics", Some("analytics")).unwrap();
        assert!(edit.bytes.contains("// project config"));
        assert!(edit.bytes.contains("// routes live under src/routes"));
        assert!(edit.bytes.contains("framework: { adapter: 'react' }"));
        assert!(edit.bytes.contains("routes: { dir: 'src/routes' }"));
    }

    #[test]
    fn appends_to_an_existing_modules_array() {
        let src = "import { defineConfig } from '@oxide-fw/core'\n\
export default defineConfig({\n  modules: ['@oxide/tailwind'],\n})\n";
        let edit = add_module_to_config(src, "@oxide/analytics", Some("analytics")).unwrap();
        assert!(edit.bytes.contains("@oxide/tailwind"));
        assert!(edit.bytes.contains("@oxide/analytics"));
        assert!(
            edit.bytes
                .contains("modules: ['@oxide/tailwind'], '@oxide/analytics'")
        );
    }

    #[test]
    fn refuses_a_dynamic_config_with_two_line_fix() {
        let src = "export default JSON.parse(readFileSync('./cfg.json', 'utf8'))\n";
        let err = add_module_to_config(src, "@oxide/analytics", Some("analytics")).unwrap_err();
        let msg = err.0.message.clone();
        assert!(msg.contains("MOD-CONFIG-DYNAMIC"), "{msg}");
        assert!(
            err.0
                .fix
                .as_ref()
                .unwrap()
                .contains("add `modules: ['@oxide/analytics']`")
        );
        assert!(
            err.0
                .fix
                .as_ref()
                .unwrap()
                .contains("convert the config to a static")
        );
    }

    #[test]
    fn pm_detection_table_distinguishes_lockfiles() {
        assert_eq!(detect_pm_from_root(std::path::Path::new("/")), PmKind::Npm);
        // The lockfile-name detection is the unit under test; pin each
        // spelling against the public enum so a new lockfile name does
        // not silently fall through to npm.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_pm_from_root(tmp.path()), PmKind::Pnpm);
        std::fs::remove_file(tmp.path().join("pnpm-lock.yaml")).unwrap();
        std::fs::write(tmp.path().join("package-lock.json"), "").unwrap();
        assert_eq!(detect_pm_from_root(tmp.path()), PmKind::Npm);
        std::fs::remove_file(tmp.path().join("package-lock.json")).unwrap();
        std::fs::write(tmp.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect_pm_from_root(tmp.path()), PmKind::Yarn);
        std::fs::remove_file(tmp.path().join("yarn.lock")).unwrap();
        std::fs::write(tmp.path().join("bun.lockb"), "").unwrap();
        assert_eq!(detect_pm_from_root(tmp.path()), PmKind::Bun);
    }
}

/// §V2-E06: the package manager detected from the project's lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmKind {
    Pnpm,
    Npm,
    Yarn,
    Bun,
}

/// The install command for a detected PM (the args the `add` command runs).
#[must_use]
pub fn pm_install_command(pm: PmKind, module: &str) -> (&'static str, Vec<String>) {
    match pm {
        PmKind::Pnpm => ("pnpm", vec!["add".to_string(), module.to_string()]),
        PmKind::Npm => ("npm", vec!["install".to_string(), module.to_string()]),
        PmKind::Yarn => ("yarn", vec!["add".to_string(), module.to_string()]),
        PmKind::Bun => ("bun", vec!["add".to_string(), module.to_string()]),
    }
}

/// Detect the project's package manager from its lockfile. Precedence:
/// pnpm > yarn > bun > npm (npm is the fallback, including a project
/// with no lockfile).
#[must_use]
pub fn detect_pm_from_root(root: &std::path::Path) -> PmKind {
    if root.join("pnpm-lock.yaml").is_file() {
        PmKind::Pnpm
    } else if root.join("yarn.lock").is_file() {
        PmKind::Yarn
    } else if root.join("bun.lockb").is_file() {
        PmKind::Bun
    } else if root.join("package-lock.json").is_file() {
        PmKind::Npm
    } else {
        PmKind::Npm
    }
}
