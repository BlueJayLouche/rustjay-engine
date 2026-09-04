//! Lenient parsing of the ISF header — the JSON in the `/* */` comment at the
//! top of a shader.
//!
//! `isf::parse` hands the comment straight to `serde_json`, which is strict.
//! A real corpus is not: of 1419 stock ISF/Shadertoy conversions, 425 were
//! rejected before a line of GLSL was looked at, and 418 of those for two
//! reasons that say nothing about whether the shader works — an `IMPORTED`
//! written as an array, and trailing commas.
//!
//! So a header that `isf::parse` accepts is passed straight through, and only
//! one it rejects is repaired and retried. Add new repairs to [`clean_json`]
//! (for what `serde_json` itself will not read) or [`repair`] (for what it
//! reads but `Isf` will not accept).

use serde_json::{Map, Value};

/// Parse a shader's ISF header, repairing the malformed-but-common shapes.
///
/// The error is already formatted for display: these reach a log line or a
/// shader-error banner, never a match arm.
pub fn parse(glsl_src: &str) -> Result<isf::Isf, String> {
    // A header that is already valid costs one parse and no repair, so the
    // common case is unaffected by anything below.
    match isf::parse(glsl_src) {
        Ok(isf) => return Ok(isf),
        Err(isf::ParseError::MissingTopComment) => {
            return Err("no `/* */` header comment at the top of the file".into());
        }
        Err(isf::ParseError::Json { .. }) => {}
    }

    let comment = top_comment(glsl_src).ok_or("no `/* */` header comment")?;
    let mut value: Value =
        serde_json::from_str(&clean_json(comment)).map_err(|e| format!("header JSON: {e}"))?;
    repair(&mut value);
    serde_json::from_value(value).map_err(|e| format!("header JSON: {e}"))
}

/// The contents of the leading `/* */`, matching `isf::parse`'s own rule.
fn top_comment(glsl_src: &str) -> Option<&str> {
    let start = glsl_src.find("/*")? + "/*".len();
    let end = start + glsl_src[start..].find("*/")?;
    Some(glsl_src[start..end].trim())
}

/// Strip what `serde_json` will not read at all: `//` comments and trailing
/// commas. String literals are left alone.
///
/// Block comments are not handled because they cannot occur: [`top_comment`]
/// ends the header at the first `*/`, so that terminator is always the header's
/// own.
fn clean_json(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            // Newlines are kept so the error's line number still means
            // something against the file the user is looking at.
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ',' => {
                let mut lookahead = chars.clone();
                let mut skipped = String::new();
                let trailing = loop {
                    match lookahead.peek() {
                        Some(c) if c.is_whitespace() => {
                            skipped.push(*c);
                            lookahead.next();
                        }
                        Some(']' | '}') => break true,
                        _ => break false,
                    }
                };
                if trailing {
                    out.push_str(&skipped);
                    chars = lookahead;
                } else {
                    out.push(',');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Fix what `serde_json` reads but [`isf::Isf`] will not accept.
fn repair(root: &mut Value) {
    let Some(root) = root.as_object_mut() else {
        return;
    };

    // `IMPORTED` is a map of name → import in the spec, but Shadertoy
    // conversions write an array: `[]` for none, otherwise entries carrying
    // their own `NAME`. Re-key them and the imports survive the translation.
    if let Some(imported) = root.get_mut("IMPORTED")
        && let Some(entries) = imported.as_array()
    {
        let mut map = Map::new();
        for (i, entry) in entries.iter().enumerate() {
            let name = entry
                .get("NAME")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("imported{i}"));
            let mut body = entry.clone();
            if let Some(body) = body.as_object_mut() {
                body.remove("NAME");
                // A cubemap import lists its six faces under one `PATH`, but an
                // import holds a single path. Keep the first face: the cube is
                // not representable here either way, and the alternative is
                // losing the whole shader over its skybox.
                if let Some(path) = body.get_mut("PATH")
                    && let Some(faces) = path.as_array()
                    && let Some(first) = faces.first().cloned()
                {
                    *path = first;
                }
            }
            map.insert(name, body);
        }
        *imported = Value::Object(map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(json: &str) -> String {
        format!("/*\n{json}\n*/\nvoid main() {{ gl_FragColor = vec4(1.0); }}")
    }

    #[test]
    fn a_valid_header_is_passed_straight_through() {
        let isf = parse(&header(r#"{ "DESCRIPTION": "fine", "INPUTS": [] }"#)).unwrap();

        assert_eq!(isf.description.as_deref(), Some("fine"));
    }

    #[test]
    fn a_missing_header_says_so_rather_than_blaming_the_json() {
        let err = parse("void main() {}").unwrap_err();

        assert!(err.contains("header comment"), "{err}");
    }

    #[test]
    fn an_empty_imported_array_means_no_imports() {
        let isf = parse(&header(r#"{ "IMPORTED": [ ] }"#)).unwrap();

        assert!(isf.imported.is_empty());
    }

    #[test]
    fn an_imported_array_keeps_its_entries_under_their_names() {
        let isf = parse(&header(
            r#"{ "IMPORTED": [
                 { "NAME": "iChannel0", "PATH": "tex01.jpg" },
                 { "NAME": "iChannel1", "PATH": "tex02.png" }
               ] }"#,
        ))
        .unwrap();

        assert_eq!(isf.imported.len(), 2);
        assert_eq!(isf.imported["iChannel0"].path, std::path::Path::new("tex01.jpg"));
        assert_eq!(isf.imported["iChannel1"].path, std::path::Path::new("tex02.png"));
    }

    #[test]
    fn a_cubemap_import_keeps_its_first_face() {
        let isf = parse(&header(
            r#"{ "IMPORTED": [
                 { "NAME": "sky", "TYPE": "cube",
                   "PATH": [ "px.png", "nx.png", "py.png" ] }
               ] }"#,
        ))
        .unwrap();

        assert_eq!(isf.imported["sky"].path, std::path::Path::new("px.png"));
    }

    #[test]
    fn trailing_commas_are_tolerated() {
        let isf = parse(&header(
            r#"{ "CATEGORIES": [ "Generator", ], "DESCRIPTION": "x", }"#,
        ))
        .unwrap();

        assert_eq!(isf.categories, ["Generator"]);
    }

    #[test]
    fn line_comments_in_the_header_are_tolerated() {
        let isf = parse(&header(
            "{\n  // which shader this is\n  \"DESCRIPTION\": \"x\"\n}",
        ))
        .unwrap();

        assert_eq!(isf.description.as_deref(), Some("x"));
    }

    #[test]
    fn a_comma_inside_a_string_is_not_mistaken_for_a_trailing_one() {
        let isf = parse(&header(r#"{ "DESCRIPTION": "a, b, c" }"#)).unwrap();

        assert_eq!(isf.description.as_deref(), Some("a, b, c"));
    }

    #[test]
    fn a_header_that_is_broken_for_any_other_reason_still_fails() {
        let err = parse(&header(r#"{ "INPUTS": [ { "NAME" "x" } ] }"#)).unwrap_err();

        assert!(err.contains("header JSON"), "{err}");
    }
}
