//! Memory-pressure helpers for chaos testing.
//!
//! These helpers allocate (and hold) bytes to simulate a system
//! under memory pressure. They DO NOT trigger OOM; the OS or Rust
//! allocator will respond to allocation failure normally. They DO
//! reliably consume the requested amount of process memory while
//! the returned guard is live.
//!
//! ## Use cases
//!
//! - Verify that retry loops handle `Err` from allocation-prone paths.
//! - Exercise code that consults available memory before issuing
//!   large operations (e.g. buffered I/O, image decoding).
//! - Pair with [`crate::FailureSchedule`] to simulate "memory
//!   pressure → operation fails" scenarios deterministically.
//!
//! ## Limits
//!
//! - This is *user-space* pressure. It cannot simulate kernel-level
//!   exhaustion (NUMA, cgroups, swap thrashing) — for that you need
//!   OS-level fault injection, which is out of scope for `dev-chaos`.
//! - The allocation is held in `Vec<u8>` form; the OS may compress,
//!   page out, or otherwise account for it differently than
//!   "wasted" memory in real workloads.
//! - Released on drop, like every other RAII guard in the suite.

/// A memory-pressure guard that holds `size_bytes` of allocated memory
/// for its entire lifetime.
///
/// Drop the guard to release the memory.
///
/// # Example
///
/// ```
/// use dev_chaos::memory_pressure::MemoryPressure;
///
/// {
///     let _hold = MemoryPressure::allocate(1_024 * 1_024); // 1 MiB
///     // ... run code under memory pressure ...
/// } // memory released here
/// ```
pub struct MemoryPressure {
    _bytes: Vec<u8>,
    /// How many bytes were requested. Stored for diagnostics.
    size_bytes: usize,
}

impl MemoryPressure {
    /// Allocate and hold `size_bytes` of zeroed memory.
    ///
    /// Every page of the allocation is written once, so the memory is
    /// resident (counted in RSS / the working set) while the guard
    /// lives, not just reserved address space.
    ///
    /// Returns the guard; drop it to release the memory. Like any `Vec`
    /// allocation, this aborts the process if the allocator cannot
    /// satisfy the request. For testing OOM / allocation-failure paths
    /// specifically, see [`MemoryPressure::try_allocate`].
    pub fn allocate(size_bytes: usize) -> Self {
        // `vec![0; n]` gets zeroed pages from the OS lazily: before the
        // pages were touched, a 512 MiB guard added nothing to RSS.
        let mut bytes = vec![0u8; size_bytes];
        touch_pages(&mut bytes);
        Self {
            _bytes: bytes,
            size_bytes,
        }
    }

    /// Attempt to allocate `size_bytes`, returning the error path
    /// rather than panicking on allocator failure.
    ///
    /// This catches `try_reserve_exact` errors. For genuinely OOM
    /// scenarios on most platforms the kernel may kill the process
    /// before this returns; honest behavior is platform-specific.
    ///
    /// On success the memory is resident, as with
    /// [`allocate`](Self::allocate).
    pub fn try_allocate(size_bytes: usize) -> Result<Self, std::collections::TryReserveError> {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.try_reserve_exact(size_bytes)?;
        bytes.resize(size_bytes, 0);
        touch_pages(&mut bytes);
        Ok(Self {
            _bytes: bytes,
            size_bytes,
        })
    }

    /// The number of bytes this guard is holding.
    pub fn size_bytes(&self) -> usize {
        self.size_bytes
    }

    /// Convenience: allocate `kib` KiB.
    pub fn allocate_kib(kib: usize) -> Self {
        Self::allocate(kib.saturating_mul(1024))
    }

    /// Convenience: allocate `mib` MiB.
    pub fn allocate_mib(mib: usize) -> Self {
        Self::allocate(mib.saturating_mul(1024 * 1024))
    }
}

/// Write one byte in every 4 KiB of `bytes` so the OS backs each page
/// with real memory. 4 KiB is the smallest common page size, so this
/// also covers systems with larger pages.
///
/// The writes are volatile so the compiler cannot drop them: storing
/// zero into memory it knows is zeroed, or into an allocation that is
/// never read, would otherwise be removed as dead.
fn touch_pages(bytes: &mut [u8]) {
    const STRIDE: usize = 4096;
    let ptr = bytes.as_mut_ptr();
    let mut i = 0;
    while i < bytes.len() {
        // SAFETY: `i < bytes.len()`, so `ptr.add(i)` is in bounds of the
        // exclusively borrowed slice and valid for a one-byte write.
        unsafe { std::ptr::write_volatile(ptr.add(i), 0) };
        i += STRIDE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_holds_requested_bytes() {
        let g = MemoryPressure::allocate(64 * 1024);
        assert_eq!(g.size_bytes(), 64 * 1024);
    }

    #[test]
    fn try_allocate_succeeds_for_small_allocations() {
        let g = MemoryPressure::try_allocate(4 * 1024).unwrap();
        assert_eq!(g.size_bytes(), 4 * 1024);
    }

    #[test]
    fn allocate_kib_and_mib_helpers() {
        let g_kib = MemoryPressure::allocate_kib(8);
        assert_eq!(g_kib.size_bytes(), 8 * 1024);
        let g_mib = MemoryPressure::allocate_mib(1);
        assert_eq!(g_mib.size_bytes(), 1024 * 1024);
    }

    #[test]
    fn dropping_guard_releases_memory() {
        // We can't reliably observe RSS in a unit test, but we can
        // confirm the guard is droppable and yields no panic.
        {
            let _g = MemoryPressure::allocate(16 * 1024);
        }
        // No panic, no leak detectable here.
    }

    #[test]
    fn touch_pages_keeps_contents_zero_and_handles_edges() {
        let mut empty: [u8; 0] = [];
        touch_pages(&mut empty);
        let mut odd = vec![0u8; 4096 * 3 + 17];
        touch_pages(&mut odd);
        assert!(odd.iter().all(|&b| b == 0));
        let g = MemoryPressure::allocate(0);
        assert_eq!(g.size_bytes(), 0);
    }

    #[test]
    fn try_allocate_huge_returns_err() {
        // Request something so large the OS will refuse to reserve.
        // usize::MAX always fails.
        let r = MemoryPressure::try_allocate(usize::MAX);
        assert!(r.is_err());
    }
}
