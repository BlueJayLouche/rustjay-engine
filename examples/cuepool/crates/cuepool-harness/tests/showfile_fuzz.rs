use cuepool_core::ShowFile;
use cuepool_harness::rng::Xorshift64;
use serde_json::Value;

/// A malformed or hostile .qproj must be REJECTED, never panic. CuePool runs
/// with `panic = "abort"` in release: a panic here is not an error dialog, it
/// is the show process dying.
#[test]
fn arbitrary_bytes_never_panic_the_showfile_parser() {
    let mut rng = Xorshift64::new(0xDEC0DE);
    for iter in 0..20_000 {
        let len = rng.next_range(0, 512) as usize;
        let bytes = rng.next_bytes(len);
        // Result is irrelevant — not panicking is the whole assertion.
        let _ = serde_json::from_slice::<ShowFile>(&bytes);
        if iter % 5_000 == 0 {
            eprintln!("showfile byte fuzz: {iter} iterations");
        }
    }
}

/// Structure-aware pass: start from a VALID show file and corrupt one field at a
/// time. Random bytes almost never reach the migration code — this does.
#[test]
fn corrupted_valid_showfiles_never_panic() {
    let base = serde_json::to_value(ShowFile::default()).expect("default ShowFile must serialize");
    let mut rng = Xorshift64::new(0xC0FFEE11);

    for _ in 0..10_000 {
        let mut v = base.clone();
        corrupt(&mut v, &mut rng, 0);
        let raw = v.clone();
        if let Ok(mut sf) = serde_json::from_value::<ShowFile>(v) {
            // Migration reads the RAW json alongside the parsed struct — the
            // combination is what the real loader does.
            cuepool_core::showfile::migration::upgrade_show_file(&mut sf, &raw);
        }
    }
}

/// Replace one randomly chosen node with a hostile value.
fn corrupt(v: &mut Value, rng: &mut Xorshift64, depth: u32) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Object(map) => {
            let n = map.len();
            if n == 0 {
                return;
            }
            let idx = rng.next_range(0, n as u32) as usize;
            if let Some((_, val)) = map.iter_mut().nth(idx) {
                if rng.next_range(0, 2) == 0 {
                    *val = hostile(rng);
                } else {
                    corrupt(val, rng, depth + 1);
                }
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                return;
            }
            let idx = rng.next_range(0, arr.len() as u32) as usize;
            if rng.next_range(0, 2) == 0 {
                arr[idx] = hostile(rng);
            } else {
                corrupt(&mut arr[idx], rng, depth + 1);
            }
        }
        other => *other = hostile(rng),
    }
}

/// Values chosen to break assumptions: NaN-ish strings, huge numbers, negatives,
/// empty containers, deep nesting, and the `$type` discriminator the cue enum
/// dispatches on.
fn hostile(rng: &mut Xorshift64) -> Value {
    match rng.next_range(0, 9) {
        0 => Value::Null,
        1 => Value::Bool(true),
        2 => Value::String(String::new()),
        3 => Value::String("\u{0}\u{feff}../../etc/passwd".into()),
        4 => Value::Number(serde_json::Number::from(-1i64)),
        5 => Value::Number(serde_json::Number::from(u64::MAX)),
        6 => Value::Array(vec![]),
        7 => Value::Object(serde_json::Map::new()),
        _ => Value::String("NotACueType".into()),
    }
}
