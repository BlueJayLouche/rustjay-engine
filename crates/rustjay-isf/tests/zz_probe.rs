use std::collections::BTreeMap;


#[test]
fn probe() {
    // Same gate as `corpus_compile_rate`: without a corpus there is nothing
    // to bucket, and `cargo test --workspace` must not fail for want of one.
    let Ok(dir) = std::env::var("ISF_CORPUS_DIR") else {
        eprintln!("ISF_CORPUS_DIR not set; skipping the header-failure breakdown");
        return;
    };
    let mut buckets: BTreeMap<String, Vec<String>> = Default::default();
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("fs") { continue; }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        n += 1;
        if let Err(err) = rustjay_isf::header::parse(&src) {
            let msg = err.to_string();
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
