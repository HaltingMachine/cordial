//! Stub implementations for every Android symbol Cordial does not provide yet.
//!
//! A stub records that it was called and returns zero. That makes the first
//! launch attempt produce a *prioritised* list of what to implement — which is
//! considerably more useful than a list of everything Roblox references.
//!
//! `CORDIAL_STUB_ABORT=1` aborts on the first hit instead, for bisecting a
//! specific failure.

use std::sync::Mutex;

include!(concat!(env!("OUT_DIR"), "/generated_stubs.rs"));

struct Hits {
    /// Call count per symbol index.
    counts: Vec<u32>,
    /// Symbol indices in first-hit order.
    order: Vec<usize>,
}

static HITS: Mutex<Option<Hits>> = Mutex::new(None);

fn abort_on_hit() -> bool {
    std::env::var_os("CORDIAL_STUB_ABORT").is_some()
}

fn quiet() -> bool {
    std::env::var_os("CORDIAL_STUB_QUIET").is_some()
}

/// Called by every generated stub.
pub fn hit(index: usize) -> i64 {
    let first = {
        let mut guard = HITS.lock().unwrap_or_else(|e| e.into_inner());
        let hits = guard.get_or_insert_with(|| Hits {
            counts: vec![0; SYMBOLS.len()],
            order: Vec::new(),
        });
        let first = hits.counts[index] == 0;
        hits.counts[index] += 1;
        if first {
            hits.order.push(index);
        }
        first
    };

    if first && !quiet() {
        eprintln!("[stub] {}", SYMBOLS[index].0);
    }
    if abort_on_hit() {
        eprintln!("[stub] CORDIAL_STUB_ABORT set — aborting on {}", SYMBOLS[index].0);
        report();
        std::process::abort();
    }
    0
}

/// Every stub called at least once, most-called first. This is the work queue.
pub fn report() {
    let guard = HITS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(hits) = guard.as_ref() else {
        eprintln!("\n=== no stubs were called ===");
        return;
    };

    let mut called: Vec<(usize, u32)> = hits
        .order
        .iter()
        .map(|&i| (i, hits.counts[i]))
        .collect();
    called.sort_by(|a, b| b.1.cmp(&a.1));

    eprintln!(
        "\n=== stubs called: {} distinct of {} ===",
        called.len(),
        SYMBOLS.len()
    );
    for (i, count) in called {
        eprintln!("  {count:>9}  {}", SYMBOLS[i].0);
    }
}
