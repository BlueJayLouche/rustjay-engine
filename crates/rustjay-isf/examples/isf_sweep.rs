//! Stage-1 sweep: walk a corpus of ISF `.fs` files, compile each through the
//! rustjay-isf pipeline, compute static performance-heuristic metrics from the
//! GLSL source, and write a JSON report.
//!
//! Usage: `cargo run --release -p rustjay-isf --example isf_sweep -- <corpus_dir> <out_json>`

use std::path::{Path, PathBuf};

/// Parse + compile one shader file. Same pattern as tests/batch_compile.rs.
fn compile_one(src: &str) -> Result<rustjay_isf::Transpiled, String> {
    let isf = rustjay_isf::header::parse(src).map_err(|e| format!("header-parse: {e}"))?;
    rustjay_isf::generate_wgsl(&isf, src)
}

/// Same categorization as tests/batch_compile.rs so results stay comparable.
fn categorize(e: &str) -> &'static str {
    if e.starts_with("read") {
        "read"
    } else if e.starts_with("header-parse") {
        "header-parse"
    } else if e.contains("overloaded functions must have the same parameter precision") {
        "builtin-fn-redefine"
    } else if e.contains("NotIOShareableType") {
        "naga-array-varying"
    } else if e.contains("ExpressionAlreadyInScope") {
        "naga-overload-bug"
    } else if e.starts_with("naga") {
        "naga-other"
    } else if e.starts_with("wgsl") {
        "wgsl"
    } else {
        "shaderc"
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            // .git is noise; checked/ is the future output folder.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name != ".git" && name != "checked" {
                collect(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("fs") {
            out.push(path);
        }
    }
}

/// Static performance heuristics counted straight from the GLSL source.
/// Rough substring counting, not a GLSL parser — that is deliberate.
struct Metrics {
    texture_samples: usize,
    loop_count: usize,
    loop_iterations_hint: usize,
    noise_calls: usize,
    transcendentals: usize,
    loc: usize,
}

fn count_any(src: &str, needles: &[&str]) -> usize {
    needles.iter().map(|n| src.matches(n).count()).sum()
}

/// Sum literal integer bounds in `for` loop conditions (`i < 8` → 8).
/// Non-literal bounds are ignored.
fn loop_iterations_hint(src: &str) -> usize {
    let mut sum = 0;
    let mut rest = src;
    while let Some(i) = rest.find("for") {
        rest = &rest[i + 3..];
        let Some(open) = rest.find('(') else { break };
        let Some(close) = rest[open..].find(')') else { break };
        let header = &rest[open + 1..open + close];
        // The condition is the middle `;`-separated clause.
        if let Some(cond) = header.split(';').nth(1)
            && let Some(lt) = cond.find('<')
        {
            let digits: String = cond[lt + 1..]
                .trim_start_matches('=')
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<usize>() {
                sum += n;
            }
        }
        rest = &rest[open + close..];
    }
    sum
}

/// Non-empty, non-comment line count. Tracks `/* */` blocks loosely.
fn loc(src: &str) -> usize {
    let mut n = 0;
    let mut in_block = false;
    for line in src.lines() {
        let t = line.trim();
        if in_block {
            if t.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if t.starts_with("/*") {
            if !t.contains("*/") {
                in_block = true;
            }
            continue;
        }
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        n += 1;
    }
    n
}

fn metrics(src: &str) -> Metrics {
    Metrics {
        texture_samples: count_any(
            src,
            &[
                "texture(",
                "texture2D(",
                "textureCube(",
                "IMG_THIS_PIXEL(",
                "IMG_NORM_PIXEL(",
                "IMG_PIXEL(",
                "IMG_THIS_NORM_PIXEL(",
            ],
        ),
        loop_count: count_any(src, &["for (", "for(", "while (", "while("]),
        loop_iterations_hint: loop_iterations_hint(src),
        noise_calls: count_any(src, &["noise(", "fbm(", "snoise(", "rand("]),
        transcendentals: count_any(
            src,
            &[
                "sin(",
                "cos(",
                "tan(",
                "pow(",
                "sqrt(",
                "exp(",
                "log(",
                "atan(",
                "length(",
                "normalize(",
            ],
        ),
        loc: loc(src),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let corpus = PathBuf::from(args.next().expect("usage: isf_sweep <corpus_dir> <out_json>"));
    let out_path = PathBuf::from(args.next().expect("usage: isf_sweep <corpus_dir> <out_json>"));

    let mut files = Vec::new();
    collect(&corpus, &mut files);
    files.sort();

    let mut entries = Vec::new();
    let mut ok = 0usize;
    let mut categories: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut top: Vec<(f64, String)> = Vec::new();

    for path in &files {
        let rel = path.strip_prefix(&corpus).unwrap_or(path).to_string_lossy().to_string();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Read first so metrics are available even when compilation fails.
        let src = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"));
        let result = src.as_deref().map_err(|e| e.clone()).and_then(compile_one);
        let (status, error, category) = match &result {
            Ok(_) => ("ok", None, None),
            Err(e) => ("fail", Some(e.clone()), Some(categorize(e))),
        };

        let mut entry = serde_json::json!({
            "path": rel,
            "name": name,
            "status": status,
        });
        if let Ok(src) = &src {
                let m = metrics(src);
                // ponytail: naive static-cost heuristic — substring counts
                // weighted by gut feel. Ceiling: it ignores branches, divergent
                // loops and actual GPU behavior; upgrade path is real GPU
                // timing per shader.
                let score = m.texture_samples as f64 * 10.0
                    + m.loop_count as f64 * 5.0
                    + m.loop_iterations_hint as f64 * 2.0
                    + m.noise_calls as f64 * 8.0
                    + m.transcendentals as f64
                    + m.loc as f64 * 0.1;
                let multi_pass = rustjay_isf::header::parse(src)
                    .map(|isf| isf.passes.len() > 1)
                    .ok();
                entry["texture_samples"] = m.texture_samples.into();
                entry["loop_count"] = m.loop_count.into();
                entry["loop_iterations_hint"] = m.loop_iterations_hint.into();
                entry["noise_calls"] = m.noise_calls.into();
                entry["transcendentals"] = m.transcendentals.into();
                entry["loc"] = m.loc.into();
                entry["static_score"] = ((score * 10.0).round() / 10.0).into();
                entry["multi_pass"] = multi_pass.into();
                top.push((score, rel.clone()));
        }
        match status {
            "ok" => ok += 1,
            _ => *categories.entry(category.unwrap()).or_default() += 1,
        }
        entry["error"] = error.into();
        entry["category"] = category.into();
        entries.push(entry);
    }

    let total = files.len();
    let summary = serde_json::json!({
        "total": total,
        "ok": ok,
        "fail": total - ok,
        "categories": categories,
    });
    let report = serde_json::json!({ "summary": summary, "entries": entries });

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).expect("create output dir");
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&report).unwrap())
        .expect("write report");

    println!("=== sweep {} ===", corpus.display());
    println!("{ok}/{total} OK ({:.1}%)", ok as f64 / total.max(1) as f64 * 100.0);
    let mut cats: Vec<_> = categories.iter().collect();
    cats.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (cat, n) in cats {
        println!("  [{cat}] {n}");
    }
    top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("\ntop 20 by static_score:");
    for (score, name) in top.iter().take(20) {
        println!("  {score:9.1}  {name}");
    }
    println!("\nwrote {}", out_path.display());
}
