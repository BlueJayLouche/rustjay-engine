//! Pass rate over a MadMapper materials corpus, bucketed by failure reason.
//! Gated on `MM_CORPUS_DIR` (a checkout of madmappersoftware/MadMapper-Materials).

use std::collections::BTreeMap;

#[test]
fn mm_corpus() {
    let Ok(dir) = std::env::var("MM_CORPUS_DIR") else {
        eprintln!("MM_CORPUS_DIR not set; skipping");
        return;
    };
    let mut files = Vec::new();
    collect(std::path::Path::new(&dir), &mut files);
    files.sort();

    let mut ok = 0;
    let mut buckets: BTreeMap<String, Vec<String>> = Default::default();
    // One full error per bucket: the summary says how many, this says what.
    let mut first: BTreeMap<String, String> = Default::default();
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        let r = rustjay_isf::header::parse(&src)
            .and_then(|isf| rustjay_isf::generate_wgsl(&isf, &src).map(|_| ()));
        match r {
            Ok(()) => ok += 1,
            Err(e) => {
                let b = buckets.entry(bucket(&e)).or_default();
                if b.is_empty() {
                    first.insert(bucket(&e), e.clone());
                }
                b.push(name);
            }
        }
    }
    let n = files.len();
    println!("\n=== {ok}/{n} compile ({:.0}%) ===", 100.0 * ok as f64 / n as f64);
    let mut rows: Vec<_> = buckets.iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (k, v) in rows {
        println!("{:5}  {k}", v.len());
        println!("       e.g. {}", v.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
        if let Some(full) = first.get(k) {
            println!("       {}", full.lines().take(4).collect::<Vec<_>>().join("\n       "));
        }
    }
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("fs") {
            out.push(p);
        }
    }
}

/// Collapse an error to the shape of the problem, dropping line numbers and
/// the specific identifier so like failures land in one bucket.
fn bucket(err: &str) -> String {
    let line = err
        .lines()
        .find(|l| l.contains("error:") && !l.trim().ends_with("error:"))
        .unwrap_or(err);
    let line = line.split("error:").nth(1).unwrap_or(line).trim();
    let line = match line.split_once('\'') {
        Some((_, rest)) => rest.split_once('\'').map(|(a, b)| format!("{a}: {b}")).unwrap_or(line.into()),
        None => line.to_string(),
    };
    // Identifier-specific messages differ only by the name; keep the message.
    let line = if line.contains("undeclared identifier") { "undeclared identifier".into() } else { line };
    let line = if line.contains("no matching overloaded function") { "no matching overloaded function".into() } else { line };
    line.chars().take(90).collect::<String>().trim().to_string()
}
