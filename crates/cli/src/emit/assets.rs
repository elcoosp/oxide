//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

pub(crate) fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_tree(&src_path, &dst_path)?;
        } else if src_path.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// BUG-11: turn `server.host` into a bind IP. `localhost` (and `""`) → the
/// loopback v4 address; an IP literal → itself; `0.0.0.0`/`::` → unspecified
/// (all interfaces). A hostname that is not an IP literal is a config error a
/// operator should see, but binding must still work — we warn and fall back to
/// loopback rather than fail to start.
/// SPEC-16.6: extract the `.svg?react` specifier from an import line.
/// Matches `import X from './foo.svg?react'` and `import './foo.svg?react'`
/// in either quote style, then returns the specifier (still with `?react`).
/// SPEC-16-A: extract `./hero.png?w=400,800` from an import line, returning
/// `(specifier, widths)`. Only matches relative specifiers ending in an
/// image extension (`.png`, `.jpg`, `.jpeg`, `.webp`, `.avif`, `.gif`)
/// followed by `?w=<comma-separated ints>`.
pub(crate) fn extract_img_w_spec(line: &str) -> Option<(&str, Vec<u32>)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import") {
        return None;
    }
    for q in ['"', '\''] {
        if let Some(start) = line.find(&format!("{q}.")) {
            let after = &line[start + 1..];
            if let Some(end) = after.find(q) {
                let candidate = &after[..end];
                if let Some((path, query)) = candidate.split_once("?w=") {
                    let is_img = path.ends_with(".png")
                        || path.ends_with(".jpg")
                        || path.ends_with(".jpeg")
                        || path.ends_with(".webp")
                        || path.ends_with(".avif")
                        || path.ends_with(".gif");
                    if is_img {
                        let widths: Vec<u32> = query
                            .split(',')
                            .filter_map(|s| s.trim().parse::<u32>().ok())
                            .collect();
                        if !widths.is_empty() {
                            return Some((candidate, widths));
                        }
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn extract_svg_react_spec(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import") {
        return None;
    }
    for q in ['"', '\''] {
        // Find last `q` before a closing `q`; the specifier is between them
        // ONLY if it ends with `.svg?react`.
        if let Some(start) = line.find(&format!("{q}.")) {
            let after = &line[start + 1..];
            if let Some(end) = after.find(q) {
                let candidate = &after[..end];
                if candidate.ends_with(".svg?react") {
                    return Some(candidate);
                }
            }
        }
    }
    None
}
