use honeycomb::Honeycomb;
use std::collections::HashMap;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

#[test]
fn vec_of_bytes_round_trips() {
    let mut v: Vec<u8> = Vec::new();
    for i in 0..10_000u32 {
        v.push((i % 256) as u8);
    }
    for (i, &b) in v.iter().enumerate() {
        assert_eq!(b, (i as u32 % 256) as u8);
    }
    drop(v);
}

#[test]
fn vec_of_strings_round_trips() {
    let mut v: Vec<String> = Vec::new();
    for i in 0..2_000 {
        v.push(format!("item-{i}-{}", "x".repeat(i % 37)));
    }
    for (i, s) in v.iter().enumerate() {
        assert_eq!(*s, format!("item-{i}-{}", "x".repeat(i % 37)));
    }
    drop(v);
}

#[test]
fn hashmap_round_trips() {
    let mut m: HashMap<u64, String> = HashMap::new();
    for i in 0..5_000u64 {
        m.insert(i, format!("value-{i}"));
    }
    for i in 0..5_000u64 {
        assert_eq!(m.get(&i), Some(&format!("value-{i}")));
    }
    for i in (0..5_000u64).step_by(2) {
        m.remove(&i);
    }
    assert_eq!(m.len(), 2_500);
    drop(m);
}

#[test]
fn boxed_values_and_nesting() {
    #[derive(Debug, PartialEq)]
    struct Nested {
        a: Box<u64>,
        b: Vec<Box<[u8; 64]>>,
        c: Option<Box<Nested2>>,
    }
    #[derive(Debug, PartialEq)]
    struct Nested2 {
        data: Vec<u8>,
    }

    let n = Nested {
        a: Box::new(42),
        b: (0..50).map(|i| Box::new([i as u8; 64])).collect(),
        c: Some(Box::new(Nested2 {
            data: vec![7; 1024],
        })),
    };

    assert_eq!(*n.a, 42);
    assert_eq!(n.b.len(), 50);
    assert_eq!(n.b[10][0], 10);
    assert_eq!(n.c.as_ref().unwrap().data.len(), 1024);
    drop(n);
}

#[test]
fn grow_and_shrink_vector_exercises_realloc() {
    let mut v: Vec<u64> = Vec::with_capacity(1);
    for i in 0..20_000u64 {
        v.push(i);
    }
    assert_eq!(v.len(), 20_000);
    assert_eq!(v[19_999], 19_999);

    v.truncate(10);
    v.shrink_to_fit();
    assert_eq!(v, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

    v.extend(10..15_000u64);
    assert_eq!(v.len(), 15_000);
    assert_eq!(v[14_999], 14_999);
    drop(v);
}

#[test]
fn large_allocation_bypasses_size_classes() {
    let v: Vec<u8> = vec![0xAB; 5_000_000];
    assert_eq!(v.len(), 5_000_000);
    assert!(v.iter().all(|&b| b == 0xAB));
    drop(v);
}

#[test]
fn stats_track_allocations_and_frees() {
    let before = ALLOCATOR.stats();
    {
        let _v: Vec<u8> = vec![1; 4096];
        let after_alloc = ALLOCATOR.stats();
        assert!(after_alloc.allocations > before.allocations);
        assert!(after_alloc.bytes_in_use >= before.bytes_in_use + 4096);
    }
    let after_drop = ALLOCATOR.stats();
    assert!(after_drop.frees > before.frees);
}

