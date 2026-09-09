use honeycomb::Honeycomb;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

/// A small xorshift PRNG so the stress test has no dependency on `rand`
/// and is fully deterministic across runs.
struct Xorshift(u64);
impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn thousands_of_mixed_alloc_free_cycles() {
    let baseline = ALLOCATOR.stats().bytes_in_use;
    let mut rng = Xorshift(0x2026_0821_dead_beef);
    let mut live: Vec<Vec<u8>> = Vec::new();

    for round in 0..20_000u64 {
        let choice = rng.next() % 3;
        match choice {
            0 => {
                // Allocate a new buffer with a size spanning every size
                // class plus the occasional large allocation.
                let size = 1 + (rng.next() % 8_192) as usize;
                let mut buf = vec![0u8; size];
                let fill = (rng.next() % 256) as u8;
                for b in buf.iter_mut() {
                    *b = fill;
                }
                live.push(buf);
            }
            1 if !live.is_empty() => {
                // Free a random live buffer, checking its contents first
                // to catch any corruption from a bad free-list splice.
                let idx = (rng.next() as usize) % live.len();
                let buf = live.swap_remove(idx);
                if let Some(&first) = buf.first() {
                    assert!(buf.iter().all(|&b| b == first), "corrupted buffer contents at round {round}");
                }
            }
            _ if !live.is_empty() => {
                // Grow or shrink a random live buffer, exercising realloc.
                let idx = (rng.next() as usize) % live.len();
                let new_size = 1 + (rng.next() % 8_192) as usize;
                let fill = live[idx].first().copied().unwrap_or(0);
                live[idx].resize(new_size, fill);
            }
            _ => {}
        }
    }

    drop(live);
    let after = ALLOCATOR.stats();

    // `bytes_in_use` is a process-global gauge, and honeycomb is installed as
    // the `#[global_allocator]`, so the libtest harness's own allocations
    // (result formatting, timing, output capture) flow through the same
    // counter from other threads while this test runs. The allocator itself
    // documents the gauge as "approximate under concurrency, exact at rest",
    // so asserting an exact return to `baseline` was flaky (~2/300, a harness
    // allocation of a few hundred bytes landing between the two snapshots).
    //
    // What the test actually intends is that the allocator reclaims what this
    // test frees: after dropping every buffer the test allocated, live bytes
    // return to ~baseline. A genuine dealloc leak instead leaves `bytes_in_use`
    // pinned near the peak the run reached. So assert the residual above
    // baseline is a negligible fraction of that peak growth: harness noise is
    // a few hundred bytes against a multi-hundred-KB peak, while a real leak
    // leaves the residual equal to the whole peak.
    let residual = after.bytes_in_use.saturating_sub(baseline);
    let peak_growth = after.peak_bytes.saturating_sub(baseline);
    assert!(
        residual <= peak_growth / 8,
        "bytes_in_use stayed {residual} bytes above baseline (peak growth {peak_growth}); \
         dealloc is not reclaiming freed memory"
    );
    assert!(after.allocations > 5_000, "expected heavy allocation traffic");
    assert!(after.frees > 5_000, "expected heavy free traffic");
}
