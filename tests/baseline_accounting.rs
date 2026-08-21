use honeycomb::Honeycomb;
use std::collections::HashMap;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

// This lives in its own test binary, deliberately. `bytes_in_use` is a
// single global counter shared by every #[test] fn that runs in the same
// binary, and cargo runs those concurrently by default (separate threads,
// same process, same static ALLOCATOR). A "returns to baseline" assertion
// only means something if nothing else is allocating on the same counter
// while it's being checked, so this check gets a binary to itself rather
// than sharing one with tests that make no such promise.
#[test]
fn bytes_in_use_returns_to_baseline_after_freeing_everything() {
    let baseline = ALLOCATOR.stats().bytes_in_use;

    {
        let mut v: Vec<Box<[u8; 256]>> = Vec::new();
        for i in 0..1_000 {
            v.push(Box::new([i as u8; 256]));
        }
        let mut m: HashMap<u32, String> = HashMap::new();
        for i in 0..1_000u32 {
            m.insert(i, format!("entry-{i}"));
        }
        let _big: Vec<u8> = vec![0; 2_000_000];

        let mid = ALLOCATOR.stats().bytes_in_use;
        assert!(mid > baseline, "allocations should raise bytes_in_use");
    }

    let after = ALLOCATOR.stats().bytes_in_use;
    assert_eq!(
        after, baseline,
        "bytes_in_use must return to baseline once every value above is dropped"
    );
}
