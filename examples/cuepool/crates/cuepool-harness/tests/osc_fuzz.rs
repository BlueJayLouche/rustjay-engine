use cuepool_harness::rng::Xorshift64;
use cuepool_protocols::osc::OscRouter;
use rosc::{OscMessage, OscType};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const PATTERNS: [&str; 11] = [
    "/cue/*/go",
    "/cue/[0-9]/stop",
    "/cue/{go,stop}",
    "/*",
    "/",
    "",
    "//",
    "/[",
    "/{",
    "/*/*/*/*/*/*",
    "/cue/?",
];

/// OSC arrives unauthenticated on the venue LAN (rx 9000 by default). Trie
/// construction and dispatch are CuePool's own code, including acceptance of
/// malformed-looking patterns as literal segments.
#[test]
fn arbitrary_addresses_and_patterns_never_panic_or_hang() {
    let hits = Arc::new(AtomicUsize::new(0));
    let current_addr = Arc::new(std::sync::Mutex::new(String::new()));
    let (done_tx, done_rx) = mpsc::channel();
    let watchdog_addr = Arc::clone(&current_addr);
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(Duration::from_secs(45)) == Err(mpsc::RecvTimeoutError::Timeout) {
            let addr = watchdog_addr.lock().unwrap_or_else(|e| e.into_inner());
            eprintln!("OSC router fuzz hung: patterns={PATTERNS:?}, addr={addr:?}");
            // ponytail: A stuck matcher cannot be unwound safely in-process;
            // isolate each route in a subprocess if per-case recovery is needed.
            std::process::abort();
        }
    });

    let mut router = OscRouter::new();
    for pattern in PATTERNS {
        let h = Arc::clone(&hits);
        router.subscribe(pattern, move |_msg: &OscMessage| {
            h.fetch_add(1, Ordering::Relaxed);
        });
    }

    let mut rng = Xorshift64::new(0x05C);
    for _ in 0..20_000 {
        let addr = random_addr(&mut rng);
        *current_addr.lock().unwrap_or_else(|e| e.into_inner()) = addr.clone();
        let msg = OscMessage {
            addr,
            args: random_args(&mut rng),
        };
        router.route(&msg);
    }

    done_tx.send(()).expect("watchdog must still be listening");
    watchdog.join().expect("watchdog must not panic");
}

fn random_addr(rng: &mut Xorshift64) -> String {
    let segments = rng.next_range(0, 6);
    let mut s = String::new();
    for _ in 0..segments {
        s.push('/');
        let len = rng.next_range(0, 8);
        for _ in 0..len {
            // Deliberately includes pattern metacharacters and non-ASCII.
            let alphabet = b"abc019*?[]{},/\\ \xc3\xa9";
            let idx = rng.next_range(0, alphabet.len() as u32) as usize;
            s.push(alphabet[idx] as char);
        }
    }
    s
}

fn random_args(rng: &mut Xorshift64) -> Vec<OscType> {
    (0..rng.next_range(0, 5))
        .map(|_| match rng.next_range(0, 5) {
            0 => OscType::Int(i32::MIN),
            1 => OscType::Float(f32::NAN),
            2 => OscType::String(String::new()),
            3 => {
                let len = rng.next_range(0, 64) as usize;
                OscType::Blob(rng.next_bytes(len))
            }
            _ => OscType::Nil,
        })
        .collect()
}
