use honeycomb::Honeycomb;
use std::collections::HashMap;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

fn workload() {
    // Small, uniform allocations.
    let mut small: Vec<Box<u64>> = Vec::new();
    for i in 0..2000u64 {
        small.push(Box::new(i));
    }

    // A growing vector, exercises realloc repeatedly. Pushing one at a time is
    // the point here (resize/vec! would allocate once), so the same-item push
    // is intentional.
    #[allow(clippy::same_item_push)]
    let mut growing: Vec<u8> = Vec::new();
    for _ in 0..5000 {
        growing.push(0xAB);
    }

    // Strings of varied length.
    let mut strings: Vec<String> = Vec::new();
    for i in 0..500 {
        strings.push(format!("honeycomb-entry-number-{i}-with-some-padding"));
    }

    // A hash map, mixed key/value sizes.
    let mut map: HashMap<u32, String> = HashMap::new();
    for i in 0..1000u32 {
        map.insert(i, format!("value-{i}"));
    }

    // One large allocation, skips the size classes entirely.
    let big: Vec<u8> = vec![0; 1_000_000];

    // Shrink the growing vector, exercises realloc downward.
    growing.truncate(100);
    growing.shrink_to_fit();

    println!("mid-run: {} small boxes, {} strings, {} map entries, {} big bytes",
        small.len(), strings.len(), map.len(), big.len());

    drop(small);
    drop(growing);
    drop(strings);
    drop(map);
    drop(big);
}

fn print_stats(label: &str) {
    let s = ALLOCATOR.stats();
    println!(
        "[{label}] allocations={} frees={} bytes_in_use={} peak_bytes={}",
        s.allocations, s.frees, s.bytes_in_use, s.peak_bytes
    );
}

fn main() {
    print_stats("start");
    workload();
    print_stats("after workload (everything dropped)");
}
