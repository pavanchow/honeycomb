# Honeycomb design

## Goal

Be readable. Be observable. Be correct. Speed is a distant fourth. This document
walks through the allocator the way you would want it explained before reading
`src/lib.rs`.

## Size classes

```
8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
```

Every allocation request computes:

```
required = max(layout.size(), layout.align(), 8)
```

and rounds up to the smallest size class that is `>= required`. If `required`
is bigger than 4096, there is no size class for it, and the request is a
**large allocation** handled directly by `std::alloc::System`.

Rounding by `max(size, align)` instead of just `size` is what keeps alignment
correct without any per-block metadata: since every class is a power of two,
and every class divides 4096 evenly, a block from class `C` starting at a
4096-aligned slab offset that is itself a multiple of `C` is automatically
aligned to `C`. Guaranteeing `C >= align` up front is what makes that
alignment sufficient for the request.

## Free-list layout

Each size class owns one free list: a singly linked list of blocks currently
not in use. The list is **intrusive**: while a block is free, nobody holds a
pointer into it, so its first machine word is reused to store the pointer to
the next free block (`FreeNode { next: *mut FreeNode }`). This is why every
size class must be at least `size_of::<*mut FreeNode>()` bytes, which holds
because the smallest class is 8 bytes on every 64-bit target Honeycomb runs
on.

There is no per-block header. A block's size class is never looked up from
the block itself; it is recomputed from the `Layout` the caller passes back
into `dealloc`/`realloc`, which Rust's `GlobalAlloc` contract guarantees
matches the layout used at allocation time.

## OS backing

When a size class's free list is empty, Honeycomb requests one **slab** from
`std::alloc::System`: `block_size * 64` bytes, aligned to 4096 (a page). The
slab is carved into 64 fixed-size blocks, linked into a chain, and spliced
onto the class's free list in one lock. One of the 64 is then handed back to
the caller.

Slabs are never returned to the OS. This is a deliberate simplification, the
same one many small allocators make: reclaiming pages back to the kernel
needs either per-slab occupancy tracking or a way to walk every block in a
slab and confirm it is free, and that bookkeeping earns its cost only under
sustained peak-then-shrink workloads. Honeycomb optimizes for readability, so
it leaks slabs to the process (not to the OS: everything is freed at process
exit) and says so here plainly instead of quietly.

Large allocations (bigger than 4096 bytes, or needing more than 4096-byte
alignment) never enter a size class. They go straight to `System::alloc` /
`System::dealloc` / `System::realloc`, and Honeycomb only wraps them to keep
its own stats accurate.

## Alignment strategy

Three guarantees combine to make every returned pointer valid for its layout:

1. Slabs are requested with `Layout::from_size_align(slab_size, 4096)`, so
   every slab starts on a 4096-byte boundary.
2. Every size class value divides 4096, so an offset that is a multiple of
   `block_size` inside a 4096-aligned slab is itself `block_size`-aligned.
3. `class_index` never picks a class smaller than the requested alignment,
   so the block's natural alignment is always sufficient.

Large allocations skip all of this and let `System` handle alignment
directly, since `System` already implements the full `GlobalAlloc` contract.

## Concurrency

Each size class's free list is guarded by a small hand-rolled spinlock built
on a single `AtomicBool` (see `Spinlock` in `src/lib.rs`). This is not
stylistic: `std::sync::Mutex` cannot be used inside a `#[global_allocator]`.
The first `Mutex::lock()` call on a thread lazily initializes thread-parking
state, and that initialization allocates, which calls back into the very
allocator that is still inside its own `lock()` call, recursing until the
stack overflows. A spinlock built purely from an atomic compare-exchange
loop never allocates, so it is safe to use here. (This was found the hard
way: an early version of Honeycomb using `Mutex` segfaulted on the very
first allocation of the program, before `main` even ran.)

Global counters (`allocations`, `frees`, `bytes_in_use`, `peak_bytes`) are
plain atomics with relaxed ordering. Snapshots taken through `stats()` are
therefore exact at rest and approximate under concurrent access from other
threads, same as any live counter.

## Unsafe invariants

The unsafe code in `src/lib.rs` depends on these invariants, each annotated
at its use site with a `// SAFETY:` comment:

- **No aliasing on free.** A pointer is only ever pushed onto a free list
  once per `dealloc` call, and the caller of `GlobalAlloc` guarantees a
  pointer is deallocated at most once and not used afterward. Violating this
  (a double free) corrupts the intrusive list by creating a cycle or a
  stale live reference, exactly as it would in any other allocator.
- **Layout stability.** `dealloc` and `realloc` receive the same `Layout`
  that was passed to the matching `alloc` call. Honeycomb recomputes the
  size class from that layout rather than storing it, so this guarantee
  from the `GlobalAlloc` contract is load-bearing.
- **Slab bounds.** Every block address written during `refill` is
  `slab_base + i * block_size` for `i` in `0..64`, which stays within
  `slab_size = block_size * 64` by construction.
- **Block size floor.** `FreeNode` is one pointer wide (8 bytes on 64-bit
  targets). The smallest size class is also 8 bytes, so writing a `next`
  pointer into a free block never writes past its end.
