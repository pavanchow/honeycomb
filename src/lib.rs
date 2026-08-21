//! Honeycomb: a small, readable segregated free-list global allocator.
//!
//! Honeycomb is not trying to beat jemalloc or mimalloc on speed. It exists
//! so the allocation path can be read start to finish in one file, and so it
//! can tell you, live, how much memory is actually in use. Every allocation
//! request is rounded up to a size class (a power of two from 8 bytes to 4
//! KiB). Each size class keeps its own free list of blocks carved out of
//! pages requested from the OS allocator. Anything bigger than a page-sized
//! block (4096 bytes, or any alignment above it) skips the size classes
//! entirely and goes straight to the system allocator.
//!
//! ```no_run
//! use honeycomb::Honeycomb;
//!
//! #[global_allocator]
//! static ALLOCATOR: Honeycomb = Honeycomb::new();
//!
//! fn main() {
//!     let v: Vec<u8> = vec![0; 1024];
//!     drop(v);
//!     println!("{:?}", ALLOCATOR.stats());
//! }
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Size classes, smallest to largest, each a power of two. Anything that
/// needs more than the last class (4096 bytes), or a larger alignment than
/// the last class provides, is treated as a large allocation and handed
/// straight to `System`.
const SIZE_CLASSES: [usize; 10] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

/// How many blocks worth of memory to request from the OS at once when a
/// size class runs out of free blocks. Chosen so even the smallest class
/// (8 bytes) doesn't send a slab request per allocation.
const SLAB_BLOCKS: usize = 64;

/// A minimal spinlock, used instead of `std::sync::Mutex`.
///
/// This is not a stylistic choice: a `#[global_allocator]` cannot use
/// `std::sync::Mutex` on most platforms. The first `Mutex::lock` call on a
/// thread lazily initializes thread-parking state, which allocates, which
/// calls back into this same allocator before the first lock is even held,
/// recursing forever and blowing the stack. A spinlock built only on an
/// `AtomicBool` never allocates, so it is safe to use here.
struct Spinlock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: access to `value` is only ever done while `locked` is held, and
// the guard that grants that access is not `Clone`, so this behaves like a
// normal mutex: at most one thread can read/write `value` at a time.
unsafe impl<T: Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    const fn new(value: T) -> Self {
        Spinlock {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    fn lock(&self) -> SpinlockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        SpinlockGuard { lock: self }
    }
}

struct SpinlockGuard<'a, T> {
    lock: &'a Spinlock<T>,
}

impl<'a, T> std::ops::Deref for SpinlockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the guard means `locked` is true and was set by
        // this thread, so no other thread can be inside `lock()`'s wait
        // loop concurrently accessing `value`.
        unsafe { &*self.lock.value.get() }
    }
}

impl<'a, T> std::ops::DerefMut for SpinlockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: same as `deref`, and exclusive because `&mut self` here
        // requires exclusive access to the one guard that holds the lock.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<'a, T> Drop for SpinlockGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// Node stored *inside* a free block. While a block is on the free list
/// nobody else holds a reference to it, so it is safe to reuse its first
/// word as an intrusive next-pointer. This is why every size class must be
/// at least `size_of::<*mut FreeNode>()` bytes, which holds because the
/// smallest class is 8 bytes on every platform Honeycomb targets.
#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

/// Wraps the free-list head pointer so it can live inside the spinlock (raw
/// pointers are not `Send` by default, but Honeycomb only ever touches the
/// pointer while holding the lock, so moving it across threads is fine).
struct FreeListHead(*mut FreeNode);
unsafe impl Send for FreeListHead {}

struct SizeClass {
    block_size: usize,
    free_list: Spinlock<FreeListHead>,
}

/// A point-in-time snapshot of allocator statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub allocations: u64,
    pub frees: u64,
    pub bytes_in_use: u64,
    pub peak_bytes: u64,
}

/// The Honeycomb global allocator. Safe to construct at compile time with
/// [`Honeycomb::new`].
pub struct Honeycomb {
    classes: [SizeClass; SIZE_CLASSES.len()],
    allocations: AtomicU64,
    frees: AtomicU64,
    bytes_in_use: AtomicUsize,
    peak_bytes: AtomicUsize,
}

impl Honeycomb {
    /// Builds a fresh allocator with empty free lists. `const fn` so it can
    /// be used directly in a `#[global_allocator] static`.
    pub const fn new() -> Self {
        // Can't build a `[SizeClass; N]` array from a `Default` impl in a
        // const fn, so lay it out by hand, one entry per size class.
        const fn class(block_size: usize) -> SizeClass {
            SizeClass {
                block_size,
                free_list: Spinlock::new(FreeListHead(ptr::null_mut())),
            }
        }
        Honeycomb {
            classes: [
                class(SIZE_CLASSES[0]),
                class(SIZE_CLASSES[1]),
                class(SIZE_CLASSES[2]),
                class(SIZE_CLASSES[3]),
                class(SIZE_CLASSES[4]),
                class(SIZE_CLASSES[5]),
                class(SIZE_CLASSES[6]),
                class(SIZE_CLASSES[7]),
                class(SIZE_CLASSES[8]),
                class(SIZE_CLASSES[9]),
            ],
            allocations: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            bytes_in_use: AtomicUsize::new(0),
            peak_bytes: AtomicUsize::new(0),
        }
    }

    /// Live statistics snapshot. Individual counters are read with relaxed
    /// ordering; concurrent alloc/dealloc can interleave between reads, so
    /// treat the snapshot as approximate under concurrency, exact at rest.
    pub fn stats(&self) -> Stats {
        Stats {
            allocations: self.allocations.load(Ordering::Relaxed),
            frees: self.frees.load(Ordering::Relaxed),
            bytes_in_use: self.bytes_in_use.load(Ordering::Relaxed) as u64,
            peak_bytes: self.peak_bytes.load(Ordering::Relaxed) as u64,
        }
    }

    /// Finds the smallest size class that can hold a request of this
    /// layout, or `None` if it needs to fall back to the system allocator
    /// (too big, or an alignment no size class satisfies).
    fn class_index(layout: Layout) -> Option<usize> {
        let required = layout.size().max(layout.align()).max(8);
        SIZE_CLASSES.iter().position(|&class| class >= required)
    }

    fn record_alloc(&self, size: usize) {
        self.allocations.fetch_add(1, Ordering::Relaxed);
        let now = self.bytes_in_use.fetch_add(size, Ordering::Relaxed) + size;
        self.peak_bytes.fetch_max(now, Ordering::Relaxed);
    }

    fn record_free(&self, size: usize) {
        self.frees.fetch_add(1, Ordering::Relaxed);
        self.bytes_in_use.fetch_sub(size, Ordering::Relaxed);
    }

    /// Pops a free block for `idx`, refilling the class from a fresh OS
    /// slab if the free list is empty. Returns null on OS allocation
    /// failure, same contract as `GlobalAlloc::alloc`.
    unsafe fn alloc_small(&self, idx: usize) -> *mut u8 {
        let class = &self.classes[idx];
        let block_size = class.block_size;

        let mut head = class.free_list.lock();
        if head.0.is_null() {
            drop(head);
            // SAFETY: refill only touches OS memory and the same lock, no
            // aliasing with anything the caller can observe yet.
            if !unsafe { self.refill(idx) } {
                return ptr::null_mut();
            }
            head = class.free_list.lock();
        }

        debug_assert!(!head.0.is_null());
        let node = head.0;
        // SAFETY: `node` came from a slab we carved and linked ourselves,
        // or from a previously freed block of exactly `block_size`, so
        // reading its `next` field is in-bounds and well-aligned.
        head.0 = unsafe { (*node).next };
        drop(head);

        self.record_alloc(block_size);
        node as *mut u8
    }

    /// Requests one slab from the system allocator, carves it into
    /// `block_size` chunks, and pushes all of them onto the class's free
    /// list. Returns `false` if the OS allocation failed.
    unsafe fn refill(&self, idx: usize) -> bool {
        let class = &self.classes[idx];
        let block_size = class.block_size;
        let slab_size = block_size * SLAB_BLOCKS;
        // Slabs are page-aligned so every block_size-aligned offset inside
        // them (block_size divides 4096 for every entry in SIZE_CLASSES)
        // is itself aligned to block_size, without any per-block bookkeeping.
        let slab_layout = match Layout::from_size_align(slab_size, 4096) {
            Ok(l) => l,
            Err(_) => return false,
        };

        // SAFETY: slab_layout has non-zero size (block_size >= 8) and a
        // valid alignment.
        let slab = unsafe { System.alloc(slab_layout) };
        if slab.is_null() {
            return false;
        }

        // Link every block in the slab into one chain, then splice the
        // whole chain onto the shared free list in a single lock.
        let mut chain: *mut FreeNode = ptr::null_mut();
        for i in 0..SLAB_BLOCKS {
            // SAFETY: `i * block_size` stays within `slab_size`, and the
            // slab is at least `block_size`-aligned (block_size <= 4096).
            let block = unsafe { slab.add(i * block_size) } as *mut FreeNode;
            unsafe {
                (*block).next = chain;
            }
            chain = block;
        }

        let mut head = class.free_list.lock();
        // SAFETY: `chain`'s tail already has `next == null`, matching the
        // invariant of an empty list we're splicing onto (checked below).
        if head.0.is_null() {
            head.0 = chain;
        } else {
            // Free list was refilled concurrently; walk to the end of the
            // new chain and attach the existing list after it.
            let mut tail = chain;
            unsafe {
                while !(*tail).next.is_null() {
                    tail = (*tail).next;
                }
                (*tail).next = head.0;
            }
            head.0 = chain;
        }
        true
    }

    unsafe fn dealloc_small(&self, idx: usize, ptr_in: *mut u8) {
        let class = &self.classes[idx];
        let node = ptr_in as *mut FreeNode;
        let mut head = class.free_list.lock();
        // SAFETY: `ptr_in` was handed out by `alloc_small` for this same
        // class and is not aliased (caller upholds GlobalAlloc's contract
        // that a pointer is deallocated at most once).
        unsafe {
            (*node).next = head.0;
        }
        head.0 = node;
        drop(head);
        self.record_free(class.block_size);
    }
}

impl Default for Honeycomb {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for Honeycomb {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match Self::class_index(layout) {
            // SAFETY: forwarding to the class-local allocator with the
            // class it was computed for.
            Some(idx) => unsafe { self.alloc_small(idx) },
            None => {
                // SAFETY: layout is exactly what the caller asked for and
                // came straight from `alloc`'s own contract.
                let ptr = unsafe { System.alloc(layout) };
                if !ptr.is_null() {
                    self.record_alloc(layout.size());
                }
                ptr
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match Self::class_index(layout) {
            // SAFETY: caller upholds GlobalAlloc's contract that `layout`
            // matches the one used to allocate `ptr`, so it maps to the
            // same size class it was carved from.
            Some(idx) => unsafe { self.dealloc_small(idx, ptr) },
            None => {
                self.record_free(layout.size());
                // SAFETY: same layout guarantee as above; this pointer
                // never went through a size class so System owns it.
                unsafe { System.dealloc(ptr, layout) };
            }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_idx = Self::class_index(layout);
        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(l) => l,
            Err(_) => return ptr::null_mut(),
        };
        let new_idx = Self::class_index(new_layout);

        match (old_idx, new_idx) {
            // Same size class: the block already fits, no move needed.
            // This is the one place Honeycomb beats a naive
            // realloc-by-copy.
            (Some(a), Some(b)) if a == b => ptr,
            // Both large: neither went through a size class, so this can
            // go straight to the system allocator's own realloc instead of
            // an alloc+copy+dealloc round trip. `old_idx == new_idx` can't
            // be used to detect this case: `None == None` is `true`, which
            // would wrongly treat every large-to-large resize as a no-op
            // and hand back a pointer to a block that never actually grew.
            (None, None) => {
                // SAFETY: `ptr` was allocated by `System` with `layout`
                // (it took the large-allocation path in `alloc`, so it
                // never entered a size class), and `new_size` is nonzero
                // whenever this is reached from a real `GlobalAlloc` call.
                let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
                if !new_ptr.is_null() {
                    self.record_free(layout.size());
                    self.record_alloc(new_size);
                }
                new_ptr
            }
            // Crossing between a size class and a large allocation, or
            // between two different size classes: only option is a fresh
            // allocation, copy, then free the old block.
            _ => {
                // SAFETY: new_layout has size new_size (checked above) and
                // the same alignment as the original allocation.
                let new_ptr = unsafe { self.alloc(new_layout) };
                if new_ptr.is_null() {
                    return ptr::null_mut();
                }
                let copy_len = layout.size().min(new_size);
                // SAFETY: `ptr` is valid for `layout.size()` bytes (caller
                // contract), `new_ptr` is freshly allocated for at least
                // `new_size` bytes, and the two regions cannot overlap
                // since `new_ptr` came from a separate allocation.
                unsafe {
                    ptr::copy_nonoverlapping(ptr, new_ptr, copy_len);
                    self.dealloc(ptr, layout);
                }
                new_ptr
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_index_picks_smallest_fit() {
        assert_eq!(Honeycomb::class_index(Layout::new::<u8>()), Some(0));
        assert_eq!(
            Honeycomb::class_index(Layout::from_size_align(9, 1).unwrap()),
            Some(1)
        );
        assert_eq!(
            Honeycomb::class_index(Layout::from_size_align(4096, 1).unwrap()),
            Some(9)
        );
        assert_eq!(
            Honeycomb::class_index(Layout::from_size_align(4097, 1).unwrap()),
            None
        );
        assert_eq!(
            Honeycomb::class_index(Layout::from_size_align(8, 4096).unwrap()),
            Some(9)
        );
    }

    #[test]
    fn stats_start_at_zero() {
        let a = Honeycomb::new();
        assert_eq!(a.stats(), Stats::default());
    }
}
