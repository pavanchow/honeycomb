use honeycomb::Honeycomb;
use std::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

// Honeycomb's accounting invariant: every dealloc reverses exactly what its
// matching alloc added to bytes_in_use. We verify that by calling the allocator
// directly, so the measured window holds nothing but our own alloc/dealloc
// pairs. A previous version drove this through a HashMap + format! workload and
// asserted an exact return to baseline, but std collections make one-time,
// persistent first-use allocations (hash seeds, output buffers) that land on
// the same global counter inside the window on some platforms, which made the
// exact-equality assertion pass on macOS and fail on Linux. Calling the
// allocator directly removes that noise and tests the real invariant portably.
#[test]
fn every_dealloc_reverses_its_alloc_across_size_classes() {
    // A spread of sizes that exercises several small size classes and the
    // large-allocation fallback to the system allocator.
    let sizes = [8usize, 16, 64, 200, 256, 1000, 4096, 100_000, 3_000_000];
    let base = ALLOCATOR.stats().bytes_in_use;

    // Fixed stack array, so nothing but the direct alloc calls touches the heap.
    let mut ptrs = [std::ptr::null_mut::<u8>(); 9];
    assert_eq!(sizes.len(), ptrs.len());

    unsafe {
        for (i, &s) in sizes.iter().enumerate() {
            let layout = Layout::from_size_align(s, 8).unwrap();
            ptrs[i] = ALLOCATOR.alloc(layout);
            assert!(!ptrs[i].is_null(), "alloc of {s} bytes returned null");
        }
        assert!(
            ALLOCATOR.stats().bytes_in_use > base,
            "allocations should raise bytes_in_use above the baseline"
        );
        for (i, &s) in sizes.iter().enumerate() {
            let layout = Layout::from_size_align(s, 8).unwrap();
            ALLOCATOR.dealloc(ptrs[i], layout);
        }
    }

    assert_eq!(
        ALLOCATOR.stats().bytes_in_use,
        base,
        "bytes_in_use must return to exactly its pre-alloc value once every block is freed"
    );
}
