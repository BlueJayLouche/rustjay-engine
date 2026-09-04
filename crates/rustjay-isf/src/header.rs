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

/// One entry of a MadMapper `GENERATORS` block: a named `float` uniform the
/// host drives, rather than a control the user sets.
///
/// `params` values are either a literal or the name of another input to read
/// the parameter from — see [`crate::compile`], which turns these into GLSL.
#[derive(Clone, Debug, PartialEq)]
pub struct Generator {
    pub name: String,
    pub ty: String,
    pub params: Map<String, Value>,
}

/// The `GENERATORS` a shader declares, empty for a shader with none.
///
/// Parsed separately from [`parse`] because `isf::Isf` has nowhere to put
/// them; a header that will not parse at all simply has no generators.
pub fn generators(glsl_src: &str) -> Vec<Generator> {
    let Some(comment) = top_comment(glsl_src) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&clean_json(comment)) else {
        return Vec::new();
    };
    root.get("GENERATORS")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    Some(Generator {
                        name: e.get("NAME")?.as_str()?.to_owned(),
                        ty: e.get("TYPE")?.as_str()?.to_owned(),
                        params: e
                            .get("PARAMS")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The contents of the leading `/* */`, matching `isf::parse`'s own rule.
fn top_comment(glsl_src: &str) -> Option<&str> {
    let start = glsl_src.find("/*")? + "/*".len();
    let end = start + glsl_src[start..].find("*/")?;
    Some(glsl_src[start..end].trim())
}

/// Strip what `serde_json` will not read at all: `//` comments, trailing
/// commas, and GLSL-style float literals (`360.`, `.5`). String literals are
/// left alone.
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
            // JSON wants a digit either side of the point; a shader author
            // writing `"MAX": 360.` is spelling it the way GLSL does.
            '.' => {
                let prev_is_digit = out.chars().last().is_some_and(|c| c.is_ascii_digit());
                let next_is_digit = chars.peek().is_some_and(char::is_ascii_digit);
                match (prev_is_digit, next_is_digit) {
                    (true, false) => out.push_str(".0"),
                    (false, true) => out.push_str("0."),
                    _ => out.push('.'),
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
                keep_first_cube_face(body);
            }
            map.insert(name, body);
        }
        *imported = Value::Object(map);
    }

    // The same faces, in an `IMPORTED` already written as the map ISF asks for.
    if let Some(imported) = root.get_mut("IMPORTED")
        && let Some(entries) = imported.as_object_mut()
    {
        for entry in entries.values_mut().filter_map(Value::as_object_mut) {
            keep_first_cube_face(entry);
        }
    }

    // Inputs get their values written in whatever the authoring tool felt
    // like: float bounds on an integer, `"True"` for a boolean.
    if let Some(inputs) = root.get_mut("INPUTS")
        && let Some(inputs) = inputs.as_array_mut()
    {
        for input in inputs.iter_mut().filter_map(Value::as_object_mut) {
            widen_float_range(input);
            let coerce: fn(&mut Value) = match input.get("TYPE").and_then(Value::as_str) {
                Some("long" | "int") => {
                    name_the_values(input);
                    round_to_int
                }
                Some("bool" | "event") => string_to_bool,
                _ => continue,
            };
            for key in ["DEFAULT", "MIN", "MAX", "IDENTITY", "VALUES"] {
                match input.get_mut(key) {
                    Some(Value::Array(values)) => values.iter_mut().for_each(coerce),
                    Some(value) => coerce(value),
                    None => {}
                }
            }
        }
    }
}

/// MadMapper's `floatRange` is a low/high pair in one `vec2` uniform, which is
/// what ISF calls a `point2D`. Its bounds are written once for both ends.
fn widen_float_range(input: &mut Map<String, Value>) {
    if input.get("TYPE").and_then(Value::as_str) != Some("floatRange") {
        return;
    }
    input.insert("TYPE".into(), Value::from("point2D"));
    for key in ["MIN", "MAX", "IDENTITY", "DEFAULT"] {
        if let Some(value) = input.get_mut(key)
            && value.is_number()
        {
            *value = Value::from(vec![value.clone(), value.clone()]);
        }
    }
}

/// A cubemap import lists its six faces under one `PATH`, but an import holds
/// a single path. Keep the first face: the cube is not representable here
/// either way, and the alternative is losing the whole shader over its skybox.
fn keep_first_cube_face(entry: &mut Map<String, Value>) {
    if let Some(path) = entry.get_mut("PATH")
        && let Some(first) = path.as_array().and_then(|faces| faces.first()).cloned()
    {
        *path = first;
    }
}

/// ISF spells a menu input as integer `VALUES` with parallel string `LABELS`;
/// MadMapper just lists the labels in `VALUES` and names one in `DEFAULT`.
/// Split that back apart, so the uniform is the index the shader compares to.
fn name_the_values(input: &mut Map<String, Value>) {
    let Some(values) = input.get("VALUES").and_then(Value::as_array) else {
        return;
    };
    let Some(labels): Option<Vec<String>> = values
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
    else {
        return; // already integer values, in the shape ISF asks for
    };
    let index_of = |v: &Value| {
        v.as_str()
            .and_then(|s| labels.iter().position(|l| l == s))
            .map(|i| Value::from(i as i64))
    };
    for key in ["DEFAULT", "MIN", "MAX", "IDENTITY"] {
        if let Some(value) = input.get_mut(key)
            && let Some(index) = index_of(value)
        {
            *value = index;
        }
    }
    let indices: Vec<i64> = (0..labels.len() as i64).collect();
    input.entry("LABELS").or_insert_with(|| Value::from(labels));
    input.insert("VALUES".into(), Value::from(indices));
}

/// Rewrite a JSON float as the nearest integer, leaving anything else alone.
fn round_to_int(value: &mut Value) {
    if let Some(f) = value.as_f64()
        && !value.is_i64()
        && !value.is_u64()
    {
        *value = Value::from(f.round() as i64);
    }
}

/// Rewrite `"True"` / `"false"` as a boolean, leaving anything else alone.
/// Numbers already deserialize, so only the quoted spellings need this.
fn string_to_bool(value: &mut Value) {
    if let Some(s) = value.as_str() {
        match s.to_ascii_lowercase().as_str() {
            "true" => *value = Value::Bool(true),
            "false" => *value = Value::Bool(false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(json: &str) -> String {
        format!("/*\n{json}\n*/\nvoid main() {{ gl_FragColor = vec4(1.0); }}")
    }

    #[test]
    fn generators_are_read_with_their_params() {
        let gens = generators(&header(
            r#"{ "GENERATORS": [ { "NAME": "t", "TYPE": "time_base",
                 "PARAMS": { "speed": "mat_speed", "reverse": true } } ] }"#,
        ));

        assert_eq!(gens.len(), 1);
        assert_eq!(gens[0].name, "t");
        assert_eq!(gens[0].ty, "time_base");
        assert_eq!(gens[0].params["speed"], "mat_speed");
    }

    #[test]
    fn a_shader_without_generators_has_none() {
        assert!(generators(&header(r#"{ "INPUTS": [] }"#)).is_empty());
        assert!(generators("void main() {}").is_empty());
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
    fn float_bounds_on_an_int_input_are_rounded() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "steps", "TYPE": "int",
                 "MIN": 0.0, "MAX": 255.0, "DEFAULT": 5 } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Long(long) = &isf.inputs[0].ty else {
            panic!("expected a long input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!(long.input_values.min, Some(0));
        assert_eq!(long.input_values.max, Some(255));
    }

    #[test]
    fn a_float_range_becomes_the_pair_it_is() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "band", "TYPE": "floatRange",
                 "DEFAULT": [0.2, 0.8], "MIN": 0.0, "MAX": 1.0 } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Point2d(p) = &isf.inputs[0].ty else {
            panic!("expected a point2D input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!(p.default, Some([0.2, 0.8]));
        assert_eq!(p.max, Some([1.0, 1.0]));
    }

    #[test]
    fn a_menu_input_listing_its_labels_becomes_an_index() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "shape", "TYPE": "long",
                 "VALUES": ["Smooth", "In", "Cut"], "DEFAULT": "In" } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Long(long) = &isf.inputs[0].ty else {
            panic!("expected a long input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!(long.input_values.default, Some(1));
        assert_eq!(long.values, vec![0, 1, 2]);
        assert_eq!(long.labels, vec!["Smooth", "In", "Cut"]);
    }

    #[test]
    fn a_menu_input_already_written_the_isf_way_is_left_alone() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "shape", "TYPE": "long",
                 "VALUES": [0, 5], "LABELS": ["Off", "On"], "DEFAULT": 5 } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Long(long) = &isf.inputs[0].ty else {
            panic!("expected a long input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!(long.input_values.default, Some(5));
        assert_eq!(long.values, vec![0, 5]);
    }

    #[test]
    fn a_quoted_boolean_default_is_read_as_a_boolean() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "on", "TYPE": "bool", "DEFAULT": "True" } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Bool(b) = &isf.inputs[0].ty else {
            panic!("expected a bool input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!(b.default, Some(true));
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
    fn glsl_spelled_float_literals_are_read_as_numbers() {
        let isf = parse(&header(
            r#"{ "INPUTS": [ { "NAME": "x", "TYPE": "float",
                 "MIN": -360., "MAX": 360., "DEFAULT": .5 } ] }"#,
        ))
        .unwrap();

        let isf::InputType::Float(f) = &isf.inputs[0].ty else {
            panic!("expected a float input, got {:?}", isf.inputs[0].ty);
        };
        assert_eq!((f.min, f.max, f.default), (Some(-360.0), Some(360.0), Some(0.5)));
    }

    #[test]
    fn a_decimal_point_inside_a_string_is_left_alone() {
        let isf = parse(&header(r#"{ "DESCRIPTION": "v1. and .5 too" }"#)).unwrap();

        assert_eq!(isf.description.as_deref(), Some("v1. and .5 too"));
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
