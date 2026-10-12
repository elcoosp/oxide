//! Naive minifier (documented substitution for Rolldown/Lightning CSS minify, D3):
//! strips `//` and `/* */` comments (string- and template-aware, including nested
//! `${}` interpolations) and collapses blank lines outside string/template
//! literals. Real minification slots in behind the same call site.

/// Strip comments + collapse blank lines. Deterministic; comments inside
/// string/template literals are preserved verbatim.
pub fn minify(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    // mode stack: base Code; strings/templates push, `${` inside a template pushes
    // an Expr(brace-depth) frame that returns to the template on the matching `}`.
    #[derive(Clone, Copy, PartialEq)]
    enum M {
        Code,
        Expr(u32),
        Str(u8),
        Tmpl,
    }
    let mut stack: Vec<M> = vec![M::Code];
    let mut line: Vec<u8> = Vec::new(); // current output line (comment-stripped)

    let flush_line = |out: &mut Vec<u8>, line: &mut Vec<u8>, keep_newline: bool| {
        while matches!(line.last(), Some(b' ') | Some(b'\t')) {
            line.pop();
        }
        if !line.is_empty() {
            out.extend_from_slice(line);
            if keep_newline {
                out.push(b'\n');
            }
        }
        line.clear();
    };

    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == b'\n' {
            let in_lit = matches!(stack.last(), Some(M::Str(_)) | Some(M::Tmpl));
            if in_lit {
                // newline inside a string/template literal is literal content
                line.push(c);
            } else {
                flush_line(&mut out, &mut line, true); // blank lines collapse away
            }
            i += 1;
            continue;
        }
        let cur = stack[stack.len() - 1];
        match cur {
            M::Str(q) => {
                line.push(c);
                if c == b'\\' && i + 1 < n {
                    line.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if c == q {
                    stack.pop();
                }
            }
            M::Tmpl => {
                if c == b'\\' && i + 1 < n {
                    line.push(c);
                    line.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                if c == b'`' {
                    line.push(c);
                    i += 1;
                    stack.pop();
                    continue;
                }
                if c == b'$' && i + 1 < n && b[i + 1] == b'{' {
                    line.push(c);
                    line.push(b[i + 1]);
                    i += 2;
                    stack.push(M::Expr(0));
                    continue;
                }
                line.push(c);
                i += 1;
            }
            M::Expr(d) => {
                if c == b'}' && d == 0 {
                    line.push(c);
                    i += 1;
                    stack.pop();
                    continue;
                }
                let top = stack.len() - 1;
                match c {
                    b'{' => {
                        stack[top] = M::Expr(d + 1);
                        line.push(c);
                        i += 1;
                    }
                    b'}' => {
                        stack[top] = M::Expr(d - 1);
                        line.push(c);
                        i += 1;
                    }
                    b'"' | b'\'' => {
                        line.push(c);
                        i += 1;
                        stack.push(M::Str(c));
                    }
                    b'`' => {
                        line.push(c);
                        i += 1;
                        stack.push(M::Tmpl);
                    }
                    b'/' if i + 1 < n && b[i + 1] == b'/' => {
                        while i < n && b[i] != b'\n' {
                            i += 1;
                        }
                    }
                    b'/' if i + 1 < n && b[i + 1] == b'*' => {
                        i += 2;
                        while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                            i += 1;
                        }
                        i = (i + 2).min(n);
                        line.push(b' '); // keep token separation
                    }
                    _ => {
                        line.push(c);
                        i += 1;
                    }
                }
            }
            M::Code => {
                match c {
                    b'"' | b'\'' => {
                        line.push(c);
                        i += 1;
                        stack.push(M::Str(c));
                    }
                    b'`' => {
                        line.push(c);
                        i += 1;
                        stack.push(M::Tmpl);
                    }
                    b'/' if i + 1 < n && b[i + 1] == b'/' => {
                        while i < n && b[i] != b'\n' {
                            i += 1;
                        }
                    }
                    b'/' if i + 1 < n && b[i + 1] == b'*' => {
                        i += 2;
                        while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                            i += 1;
                        }
                        i = (i + 2).min(n);
                        line.push(b' '); // keep token separation
                    }
                    _ => {
                        line.push(c);
                        i += 1;
                    }
                }
            }
        }
    }
    flush_line(&mut out, &mut line, false);
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::minify;

    #[test]
    fn strips_comments_and_blank_lines_but_keeps_literals() {
        let src = "// header\nconst url = \"http://example.com\"; // trailing\n\n/* block\n   span */\nconst tpl = `line1\n\nline2`;\nconst x = 1;\n";
        let out = minify(src);
        assert_eq!(
            out,
            "const url = \"http://example.com\";\nconst tpl = `line1\n\nline2`;\nconst x = 1;\n"
        );
    }
}
