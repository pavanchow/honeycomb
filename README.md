# Honeycomb

**A small, readable memory allocator in Rust that shows you what your program is actually doing with memory.**

Honeycomb is a segregated free-list global allocator you can plug into any Rust program with `#[global_allocator]`. It is built entirely on the standard library, with no dependencies, and it is small enough to read start to finish in one sitting.

## Honest note

jemalloc and mimalloc are fast black boxes tuned by years of production traffic. Honeycomb is not trying to beat them, and it will not. It exists for the opposite reason: to be an allocator whose internals you can actually follow, and one that tells you, live, how much memory your program is using. Treat it as a teaching and observability allocator, not a drop-in replacement for a production-grade one.

## How it works

- Every allocation request is rounded up to a **size class**: a power of two from 8 bytes up to 4096 bytes.
- Each size class keeps its own **free list**, an intrusive singly linked list threaded through the free blocks themselves, so there is no bookkeeping overhead per block.
- When a size class runs dry, Honeycomb requests a page-aligned slab from the system allocator and carves it into fresh blocks for that class.
- Anything larger than 4096 bytes, or requiring more alignment than a size class provides, skips the classes entirely and goes straight to the system allocator.
- Growing or shrinking an allocation within the same size class is free: no copy, no move, just returning the same pointer.

## Live stats

Call `Honeycomb::stats()` any time to get a snapshot:

```rust
use honeycomb::Honeycomb;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();

fn main() {
    let v: Vec<u8> = vec![0; 1024];
    drop(v);
    println!("{:?}", ALLOCATOR.stats());
    // Stats { allocations: .., frees: .., bytes_in_use: .., peak_bytes: .. }
}
```

`bytes_in_use` and `peak_bytes` are the numbers you actually want when you are trying to answer "where did my memory go" without reaching for a profiler.

## Usage

Add Honeycomb as a dependency, then install it as your global allocator:

```rust
use honeycomb::Honeycomb;

#[global_allocator]
static ALLOCATOR: Honeycomb = Honeycomb::new();
```

That is the whole integration. Every `Vec`, `String`, `Box`, and `HashMap` in the program now goes through Honeycomb.

## Design

The full design writeup, including size classes, free-list layout, alignment strategy, and the unsafe invariants the allocator depends on, is in [DESIGN.md](DESIGN.md).

## Tests

```
cargo test
```

Tests install Honeycomb as the real global allocator and exercise it with `Vec<u8>`, `Vec<String>`, `HashMap`, nested `Box` structures, vector growth and shrinkage, a stress test running thousands of randomized alloc/free/resize cycles, and an accounting check that `bytes_in_use` returns to baseline once everything is freed.

By Pavan Nallamothu.
