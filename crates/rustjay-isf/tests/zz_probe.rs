use std::collections::BTreeMap;

fn strip_json_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut prev = '\0';
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '"' && prev != '\\' { in_str = false; }
        } else if c == '"' { in_str = true; out.push(c); }
        else if c == '/' && chars.peek() == Some(&'/') {
            for c2 in chars.by_ref() { if c2 == '\n' { out.push('\n'); break; } }
        } else { out.push(c); }
        prev = c;
    }
    out
}

#[test]
fn probe() {
    let dir = std::env::var("ISF_CORPUS_DIR").unwrap();
    let mut buckets: BTreeMap<String, Vec<String>> = Default::default();
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("fs") { continue; }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        n += 1;
        if let Err(err) = rustjay_isf::header::parse(&src) {
            let msg = format!("{err}");
            // Normalise away line/column and quoted specifics.
            let key = msg
                .split(" at line ").next().unwrap_or(&msg)
                .to_string();
            buckets.entry(key).or_default().push(
                p.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    let total: usize = buckets.values().map(|v| v.len()).sum();
    println!("\n=== {n} files, {total} header-parse failures ===");
    let mut rows: Vec<_> = buckets.iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (k, v) in rows {
        println!("{:5}  {k}", v.len());
        println!("         e.g. {}", v.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    }
}
