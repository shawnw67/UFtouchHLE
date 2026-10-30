/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
use super::{GuestUSize, VAddr, PAGE_SIZE};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// iPhone OS's allocator always aligns to 16 bytes at minimum, and this
/// is also the minimum allocation size.
pub const MIN_CHUNK_SIZE: GuestUSize = 16;

/// A non-empty range of bytes in virtual address space.
///
/// Similar to [`RangeInclusive<u32>`][std::ops::RangeInclusive] but with a
/// more convenient representation.
#[derive(PartialEq, Eq, Copy, Clone)]
pub struct Chunk {
    pub(super) base: VAddr,
    pub(super) size: NonZeroU32,
}

impl Chunk {
    pub fn new(base: VAddr, size: GuestUSize) -> Chunk {
        Chunk {
            base,
            size: NonZeroU32::new(size).unwrap(),
        }
    }

    fn merge(self, other: Chunk) -> Chunk {
        assert!(
            self.last_byte() + 1 == other.base || other.last_byte() + 1 == self.base,
            "Chunks must be adjacent to merge"
        );
        Chunk::new(
            self.base.min(other.base),
            self.size.get() + other.size.get(),
        )
    }

    #[inline(always)]
    fn last_byte(&self) -> VAddr {
        self.base + (self.size.get() - 1)
    }

    #[inline(always)]
    fn contains(&self, addr: VAddr) -> bool {
        self.base <= addr && addr <= self.last_byte()
    }

    #[inline(always)]
    fn trisect_by(&self, middle: Chunk) -> Option<(Option<Chunk>, Option<Chunk>)> {
        if !self.contains(middle.base) || !self.contains(middle.last_byte()) {
            return None;
        }

        Some(self.difference(middle))
    }

    #[inline(always)]
    /// Returns parts of `self` that don't overlap with other
    fn difference(&self, other: Chunk) -> (Option<Chunk>, Option<Chunk>) {
        if other.last_byte() < self.base {
            return (None, Some(*self));
        }

        if other.base > self.last_byte() {
            return (Some(*self), None);
        }

        let left = match other.base.checked_sub(self.base) {
            None | Some(0) => None,
            Some(size) => Some(Chunk::new(self.base, size)),
        };

        let right = match self.last_byte().checked_sub(other.last_byte()) {
            None | Some(0) => None,
            Some(size) => Some(Chunk::new(other.last_byte() + 1, size)),
        };

        (left, right)
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Chunk ({:#x}–{:#x}; {:#x} bytes)",
            self.base,
            self.base + (self.size.get() - 1),
            self.size.get()
        )
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::Chunk;
    #[test]
    fn contains() {
        assert!(Chunk::new(2, 4).contains(2));
        assert!(Chunk::new(2, 4).contains(5));
        assert!(!Chunk::new(2, 4).contains(6));
    }

    #[test]
    fn trisect() {
        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(3, 2)),
            Some((Some(Chunk::new(2, 1)), Some(Chunk::new(5, 1))))
        );
        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(2, 2)),
            Some((None, Some(Chunk::new(4, 2))))
        );
        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(4, 2)),
            Some((Some(Chunk::new(2, 2)), None))
        );
        assert_eq!(Chunk::new(2, 4).trisect_by(Chunk::new(1, 2)), None);
        assert_eq!(Chunk::new(2, 4).trisect_by(Chunk::new(5, 2)), None);
    }

    #[test]
    fn merge() {
        let a = Chunk::new(0, 10);
        let b = Chunk::new(10, 10);
        assert_eq!(a.merge(b), Chunk::new(0, 20));
        assert_eq!(b.merge(a), Chunk::new(0, 20));
    }

    #[test]
    #[should_panic]
    fn merge_non_adjacent() {
        let a = Chunk::new(0, 10);
        let b = Chunk::new(20, 10);
        a.merge(b);
    }

    #[test]
    fn difference() {
        let test_chunk = Chunk::new(10, 10);
        assert_eq!(
            test_chunk.difference(Chunk::new(0, 5)),
            (None, Some(Chunk::new(10, 10)))
        );

        assert_eq!(
            test_chunk.difference(Chunk::new(20, 10)),
            (Some(Chunk::new(10, 10)), None)
        );

        assert_eq!(test_chunk.difference(Chunk::new(10, 10)), (None, None));

        assert_eq!(test_chunk.difference(Chunk::new(9, 15)), (None, None));

        assert_eq!(
            test_chunk.difference(Chunk::new(5, 10)),
            (None, Some(Chunk::new(15, 5)))
        );

        assert_eq!(
            test_chunk.difference(Chunk::new(15, 10)),
            (Some(Chunk::new(10, 5)), None)
        );

        assert_eq!(
            test_chunk.difference(Chunk::new(13, 3)),
            (Some(Chunk::new(10, 3)), Some(Chunk::new(16, 4)))
        );
    }
}

/// Specialized collection types. They're kept in their own module so the
/// allocator can only access them via their public methods, so that there's
/// less places inconsistencies between the sub-collections could happen.
mod collections {
    use super::*;

    #[derive(Default, Debug)]
    pub struct ChunkMap {
        chunks: BTreeMap<VAddr, NonZeroU32>,
    }
    impl ChunkMap {
        #[inline(always)]
        pub fn insert(&mut self, Chunk { base, size }: Chunk) {
            assert!(self.chunks.insert(base, size).is_none());
        }
        #[inline(always)]
        pub fn remove_with_base(&mut self, base: VAddr) -> Option<Chunk> {
            self.chunks.remove(&base).map(|size| Chunk { base, size })
        }
        #[inline(always)]
        pub fn remove_with_end(&mut self, end: VAddr) -> Option<Chunk> {
            let (&base, &size) = self.chunks.range(..end).next_back()?;
            let chunk = Chunk { base, size };
            if chunk.last_byte() + 1 != end {
                return None;
            }
            Some(self.remove_with_base(chunk.base).unwrap())
        }
        #[inline(always)]
        pub fn get_size_with_base(&self, base: VAddr) -> Option<NonZeroU32> {
            self.chunks.get(&base).copied()
        }

        pub fn overlapping_chunks(&self, chunk: Chunk) -> impl Iterator<Item = Chunk> + '_ {
            let start = self
                .chunks
                .range(..=chunk.base)
                .next_back()
                .filter(|(&base, &size)| chunk.base < base + size.get())
                .map(|(&base, _)| base)
                .unwrap_or(chunk.base);

            self.chunks
                .range(start..=chunk.last_byte())
                .map(|(&base, size)| Chunk::new(base, size.get()))
        }

        /// Remove all chunks overlapping a provided Chunk. In the case
        /// of overlap the non overlapping portion is left
        pub fn remove_range(&mut self, remove: Chunk) {
            let bases: Vec<VAddr> = self
                .overlapping_chunks(remove)
                .map(|chunk| chunk.base)
                .collect();

            for base in bases {
                let chunk = self.remove_with_base(base).unwrap();
                let (left, right) = chunk.difference(remove);
                if let Some(left) = left {
                    self.insert(left);
                }

                if let Some(right) = right {
                    self.insert(right);
                }
            }
        }
    }

    impl IntoIterator for ChunkMap {
        type Item = Chunk;
        type IntoIter = std::iter::Map<
            std::collections::btree_map::IntoIter<VAddr, NonZeroU32>,
            fn((VAddr, NonZeroU32)) -> Self::Item,
        >;

        fn into_iter(self) -> Self::IntoIter {
            self.chunks
                .into_iter()
                .map(|(base, size)| Chunk { base, size })
        }
    }

    #[derive(Debug)]
    pub struct SizeBucketedChunkMap {
        min_chunk_size: u32,
        chunks: ChunkMap,
        chunks_by_log2_size: Vec<Vec<Chunk>>,
    }
    impl SizeBucketedChunkMap {
        pub fn new(min_chunk_size: u32) -> Self {
            Self {
                min_chunk_size,
                chunks: Default::default(),
                chunks_by_log2_size: vec![
                    Vec::new();
                    (u32::MAX.ilog2() - min_chunk_size.ilog2()) as usize + 1
                ],
            }
        }

        /// Get log2 size bucket for chunk.
        fn bucket_for(&self, size: GuestUSize) -> usize {
            (size.ilog2() - self.min_chunk_size.ilog2()) as usize
        }

        pub fn insert(&mut self, chunk: Chunk) {
            assert!(chunk.size.get() >= self.min_chunk_size);
            self.chunks.insert(chunk);
            let bucket_size = self.bucket_for(chunk.size.get());
            self.chunks_by_log2_size[bucket_size].push(chunk);
        }

        #[inline(always)]
        fn remove_from_bucket(&mut self, chunk: Chunk) {
            let bucket_size = self.bucket_for(chunk.size.get());
            let bucket = &mut self.chunks_by_log2_size[bucket_size];
            // Search from the end (recent frees are usually at the end, so
            // following the generational hypothesis, that's a better place to
            // start)
            let idx = bucket
                .iter()
                .rposition(|chunk2| chunk.base == chunk2.base)
                .unwrap();
            assert_eq!(chunk, bucket.swap_remove(idx));
        }

        pub fn remove_with_base(&mut self, base: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_base(base)?;
            self.remove_from_bucket(chunk);
            Some(chunk)
        }

        pub fn remove_with_end(&mut self, end: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_end(end)?;
            self.remove_from_bucket(chunk);
            Some(chunk)
        }

        /// Remove all chunks overlapping a provided Chunk. In the case
        /// of overlap the non overlapping portion is carved out
        pub fn remove_range(&mut self, remove: Chunk) {
            let bases: Vec<VAddr> = self
                .chunks
                .overlapping_chunks(remove)
                .map(|chunk| chunk.base)
                .collect();

            for base in bases {
                let chunk = self.remove_with_base(base).unwrap();
                let (left, right) = chunk.difference(remove);
                if let Some(left) = left {
                    self.insert(left);
                }

                if let Some(right) = right {
                    self.insert(right);
                }
            }
        }

        fn allocate_in_bucket(&mut self, size: GuestUSize, bucket: usize) -> Option<Chunk> {
            let (idx, _) = {
                let mut best_chunk: Option<(usize, GuestUSize)> = None;

                // Search from end because we should prefer recently-freed
                // allocations that might be the right size.
                for (idx, chunk) in self.chunks_by_log2_size[bucket]
                    .iter_mut()
                    .enumerate()
                    .rev()
                {
                    if chunk.size.get() >= size
                        && (best_chunk.is_none() || best_chunk.unwrap().1 > chunk.size.get())
                    {
                        best_chunk = Some((idx, chunk.size.get()));
                        if chunk.size.get() == size {
                            break;
                        }
                    }
                }

                best_chunk
            }?;

            let existing = self.chunks_by_log2_size[bucket].swap_remove(idx);
            let existing2 = self.chunks.remove_with_base(existing.base);
            assert_eq!(Some(existing), existing2);

            if existing.size.get() == size {
                return Some(existing);
            }

            let alloc = Chunk::new(existing.base, size);
            let rump = Chunk::new(existing.base + size, existing.size.get() - size);
            self.insert(rump);

            Some(alloc)
        }

        pub fn allocate(&mut self, size: GuestUSize) -> Option<Chunk> {
            assert!(size >= self.min_chunk_size);

            // Look in the smallest bucket first. This is the only bucket where
            // an exact match can be found.

            let bucket = self.bucket_for(size);
            if let Some(alloc) = self.allocate_in_bucket(size, bucket) {
                return Some(alloc);
            }

            // Exact match has been ruled out, find the smallest chunk in the
            // next largest non-empty bucket.

            let bucket = self.chunks_by_log2_size[bucket + 1..]
                .iter()
                .position(|bucket| !bucket.is_empty())?
                + bucket
                + 1;
            self.allocate_in_bucket(size, bucket)
        }

        pub fn iter(&self) -> impl Iterator<Item = Chunk> + '_ {
            self.chunks_by_log2_size
                .iter()
                .flat_map(|chunks| chunks.iter())
                .copied()
        }

        pub fn allocate_aligned(
            &mut self,
            size: GuestUSize,
            alignment: GuestUSize,
        ) -> Option<Chunk> {
            assert!(size >= self.min_chunk_size && alignment.is_power_of_two());
            let (existing, base) = self
                .iter()
                .filter_map(|chunk| {
                    let aligned = (u64::from(chunk.base) + u64::from(alignment) - 1)
                        & !(u64::from(alignment) - 1);
                    if aligned + u64::from(size)
                        <= u64::from(chunk.base) + u64::from(chunk.size.get())
                    {
                        Some((chunk, u32::try_from(aligned).ok()?))
                    } else {
                        None
                    }
                })
                .min_by_key(|(chunk, _)| (chunk.size.get(), chunk.base))?;
            let bucket = self.bucket_for(existing.size.get());
            let index = self.chunks_by_log2_size[bucket]
                .iter()
                .position(|chunk| chunk.base == existing.base)
                .unwrap();
            assert_eq!(
                self.chunks_by_log2_size[bucket].swap_remove(index),
                existing
            );
            assert_eq!(self.chunks.remove_with_base(existing.base), Some(existing));
            let alloc = Chunk::new(base, size);
            let (before, after) = existing.difference(alloc);
            if let Some(before) = before {
                self.insert(before);
            }
            if let Some(after) = after {
                self.insert(after);
            }
            Some(alloc)
        }
    }
}
use collections::{ChunkMap, SizeBucketedChunkMap};

/// Tracks which memory is in use and makes allocations from it.
#[derive(Debug)]
pub struct HeapAllocator {
    used_chunks: ChunkMap,
    unused_chunks: SizeBucketedChunkMap,
    // These are chunks that are managed by an external allocator
    external_chunks: ChunkMap,
    backing_chunks: Vec<Chunk>,
}

impl HeapAllocator {
    /// Size of chunks requested by the heap from the VM allocator.
    /// This is set to 2MiB based on the original jemalloc paper.
    pub const HEAP_CHUNK_SIZE: GuestUSize = 2 * 1024 * 1024;

    /// This is the maximum allocation for the heap. Anything else is deferred
    /// to the vm allocator. This is set to 15 KiB according to:
    /// <https://www.cocoawithlove.com/2010/05/look-at-how-malloc-works-on-mac.html>
    pub const HEAP_ALLOCATION_THRESHOLD: GuestUSize = (15 * 1024) - 1;

    pub fn new(vm: &mut VMAllocator, size: GuestUSize) -> HeapAllocator {
        Self::try_new(vm, size).expect("Failed to allocate heap space")
    }

    pub fn try_new(vm: &mut VMAllocator, size: GuestUSize) -> Option<HeapAllocator> {
        let mut unused_chunks = SizeBucketedChunkMap::new(MIN_CHUNK_SIZE);
        let mut backing_chunks = Vec::new();

        if size > 0 {
            let base_chunk = vm.allocate(None, size).ok()?;
            backing_chunks.push(base_chunk);
            unused_chunks.insert(base_chunk);
        }

        Some(HeapAllocator {
            used_chunks: Default::default(),
            unused_chunks,
            external_chunks: Default::default(),
            backing_chunks,
        })
    }

    pub fn alloc(&mut self, vm: &mut VMAllocator, size: GuestUSize) -> Option<Chunk> {
        let size = size.max(MIN_CHUNK_SIZE);
        let size = Self::align(size, MIN_CHUNK_SIZE);

        let alloc = if size > Self::HEAP_ALLOCATION_THRESHOLD {
            let alloc = vm.allocate(None, size).ok()?;
            self.external_chunks.insert(alloc);
            alloc
        } else {
            let alloc = self.unused_chunks.allocate(size).or_else(|| {
                self.grow(vm)?;
                self.unused_chunks.allocate(size)
            })?;
            self.used_chunks.insert(alloc);
            alloc
        };

        Some(alloc)
    }

    /// Special case for valloc().
    /// This does not respect [Self::HEAP_ALLOCATION_THRESHOLD]!
    pub fn valloc(&mut self, vm: &mut VMAllocator, size: GuestUSize) -> Option<Chunk> {
        assert!(size > 0); // TODO
        let size = size.next_multiple_of(PAGE_SIZE);
        let alloc = vm.allocate(None, size).ok()?;
        self.external_chunks.insert(alloc);
        Some(alloc)
    }

    /// Aligned allocations have real heap/VM records, not hidden headers.
    pub fn memalign(
        &mut self,
        vm: &mut VMAllocator,
        alignment: GuestUSize,
        size: GuestUSize,
    ) -> Option<Chunk> {
        if alignment <= MIN_CHUNK_SIZE {
            return self.alloc(vm, size);
        }
        let size = size
            .max(MIN_CHUNK_SIZE)
            .checked_next_multiple_of(MIN_CHUNK_SIZE)?;
        if size <= Self::HEAP_ALLOCATION_THRESHOLD && alignment <= Self::HEAP_CHUNK_SIZE {
            let alloc = self
                .unused_chunks
                .allocate_aligned(size, alignment)
                .or_else(|| {
                    self.grow(vm)?;
                    self.unused_chunks.allocate_aligned(size, alignment)
                });
            if let Some(alloc) = alloc {
                self.used_chunks.insert(alloc);
                return Some(alloc);
            }
        }
        let alloc = vm.allocate_aligned(alignment, size).ok()?;
        self.external_chunks.insert(alloc);
        Some(alloc)
    }

    pub fn allocated_size(&self, base: VAddr) -> Option<GuestUSize> {
        self.external_chunks
            .get_size_with_base(base)
            .or_else(|| self.used_chunks.get_size_with_base(base))
            .map(NonZeroU32::get)
    }

    pub fn is_vm_allocation(&self, base: VAddr) -> bool {
        self.external_chunks.get_size_with_base(base).is_some()
    }

    fn align(size: GuestUSize, align: GuestUSize) -> GuestUSize {
        if !size.is_multiple_of(align) {
            size + align - (size % align)
        } else {
            size
        }
    }

    /// This is used for realloc
    pub fn find_allocated_size(&mut self, base: VAddr) -> GuestUSize {
        self.allocated_size(base)
            .unwrap_or_else(|| panic!("Can't find {base:#x}, unknown allocation!"))
    }

    /// Returns the size of the freed chunk so it can be zeroed if desired
    #[must_use]
    pub fn free(&mut self, vm: &mut VMAllocator, base: VAddr) -> GuestUSize {
        if let Some(freed) = self.external_chunks.remove_with_base(base) {
            vm.deallocate(freed.base, freed.size.get());
            return freed.size.get();
        }

        let Some(freed) = self.used_chunks.remove_with_base(base) else {
            log!("Can't free {:#x}, unknown allocation!", base);
            return 0;
        };

        let mut combined = freed;
        if let Some(right) = self.unused_chunks.remove_with_base(freed.last_byte() + 1) {
            combined = combined.merge(right);
        }
        if let Some(left) = self.unused_chunks.remove_with_end(freed.base) {
            combined = combined.merge(left);
        }
        self.unused_chunks.insert(combined);

        freed.size.get()
    }

    /// Consume the allocator returning an iterator over the managed
    /// virtual memory chunks
    pub fn into_vm_chunks(self) -> impl Iterator<Item = Chunk> {
        self.external_chunks.into_iter().chain(self.backing_chunks)
    }

    fn grow(&mut self, vm: &mut VMAllocator) -> Option<()> {
        log!("Attempting to grow heap.");
        let chunk = vm.allocate(None, Self::HEAP_CHUNK_SIZE).ok()?;

        self.backing_chunks.push(chunk);
        self.unused_chunks.insert(chunk);
        Some(())
    }
}

#[derive(Debug)]
pub enum VMAllocError {
    AddressUnavailable,
    NoSpace,
}

/// Virtual Memory Allocator which handles allocation with page granularity
#[derive(Debug)]
pub struct VMAllocator {
    used_chunks: ChunkMap,
    unused_chunks: SizeBucketedChunkMap,
}

impl VMAllocator {
    pub fn new(base: VAddr, size: GuestUSize) -> VMAllocator {
        let allocation_space = Chunk::new(base, size);

        let mut unused_chunks = SizeBucketedChunkMap::new(PAGE_SIZE);
        unused_chunks.insert(allocation_space);

        VMAllocator {
            used_chunks: Default::default(),
            unused_chunks,
        }
    }

    pub fn allocate(
        &mut self,
        address: Option<VAddr>,
        size: GuestUSize,
    ) -> Result<Chunk, VMAllocError> {
        let size = size.next_multiple_of(PAGE_SIZE);
        match address {
            Some(address) => {
                let address = address & !(PAGE_SIZE - 1);
                self.allocate_at(address, size)
            }
            None => self.allocate_any(size),
        }
    }

    fn allocate_aligned(
        &mut self,
        alignment: GuestUSize,
        size: GuestUSize,
    ) -> Result<Chunk, VMAllocError> {
        assert!(alignment.is_power_of_two());
        let alignment = u64::from(alignment.max(PAGE_SIZE));
        let size = size
            .max(1)
            .checked_next_multiple_of(PAGE_SIZE)
            .ok_or(VMAllocError::NoSpace)?;
        // u64 arithmetic also handles already aligned high guest addresses.
        let address = self
            .unused_chunks
            .iter()
            .filter_map(|chunk| {
                let aligned = (u64::from(chunk.base) + alignment - 1) & !(alignment - 1);
                let end = aligned + u64::from(size);
                if end <= u64::from(chunk.base) + u64::from(chunk.size.get()) {
                    u32::try_from(aligned).ok()
                } else {
                    None
                }
            })
            .min()
            .ok_or(VMAllocError::NoSpace)?;
        self.allocate_at(address, size)
    }

    pub fn deallocate(&mut self, address: VAddr, size: GuestUSize) -> Chunk {
        // From testing vm_deallocate you can deallocate anything as long as
        // the memory is not protected. Since we have no permissions we can
        // always succeed
        let size = size.next_multiple_of(PAGE_SIZE);
        let address = address & !(PAGE_SIZE - 1);
        let freed = Chunk::new(address, size);

        self.used_chunks.remove_range(freed);
        self.unused_chunks.remove_range(freed);

        let left_adjacent = self.unused_chunks.remove_with_base(freed.last_byte() + 1);
        let right_adjacent = self.unused_chunks.remove_with_end(freed.base);

        let mut combined = freed;

        if let Some(adjacent) = left_adjacent {
            combined = combined.merge(adjacent);
        }

        if let Some(adjacent) = right_adjacent {
            combined = combined.merge(adjacent);
        }

        self.unused_chunks.insert(combined);
        freed
    }

    fn allocate_at(&mut self, address: VAddr, size: GuestUSize) -> Result<Chunk, VMAllocError> {
        assert!(address.is_multiple_of(PAGE_SIZE));
        assert!(size.is_multiple_of(PAGE_SIZE) && size >= PAGE_SIZE);
        let chunk = Chunk::new(address, size);

        let to_trisect = self
            .unused_chunks
            .iter()
            .find(|unused_chunk| unused_chunk.contains(address))
            .ok_or(VMAllocError::AddressUnavailable)?;

        let (before, after) = to_trisect.trisect_by(chunk).ok_or(VMAllocError::NoSpace)?;
        self.unused_chunks.remove_with_base(to_trisect.base);
        if let Some(before) = before {
            self.unused_chunks.insert(before);
        }
        if let Some(after) = after {
            self.unused_chunks.insert(after);
        }
        self.used_chunks.insert(chunk);

        Ok(chunk)
    }

    fn allocate_any(&mut self, size: GuestUSize) -> Result<Chunk, VMAllocError> {
        assert!(size.is_multiple_of(PAGE_SIZE) && size >= PAGE_SIZE);

        let alloc = self
            .unused_chunks
            .allocate(size)
            .ok_or(VMAllocError::NoSpace)?;

        self.used_chunks.insert(alloc);

        Ok(alloc)
    }
}

#[cfg(test)]
mod aligned_allocation_tests {
    use super::*;

    #[test]
    fn alignment_padding_remains_available_and_merges_on_free() {
        let mut vm = VMAllocator::new(PAGE_SIZE, PAGE_SIZE * 16);
        let aligned = vm.allocate_aligned(PAGE_SIZE * 16, 1).unwrap();
        assert_eq!(aligned.base, PAGE_SIZE * 16);
        assert_eq!(aligned.size.get(), PAGE_SIZE);
        let prefix = vm.allocate(Some(PAGE_SIZE), PAGE_SIZE * 15).unwrap();
        vm.deallocate(aligned.base, aligned.size.get());
        vm.deallocate(prefix.base, prefix.size.get());
        assert!(vm.allocate(Some(PAGE_SIZE), PAGE_SIZE * 16).is_ok());
    }

    #[test]
    fn high_alignment_and_rounding_failure_do_not_wrap_or_consume_space() {
        let mut vm = VMAllocator::new(0x80000000, PAGE_SIZE * 2);
        assert!(vm.allocate_aligned(0x80000000, u32::MAX).is_err());
        let aligned = vm.allocate_aligned(0x80000000, 1).unwrap();
        assert_eq!(aligned.base, 0x80000000);
        assert_eq!(aligned.size.get(), PAGE_SIZE);
        assert!(vm.allocate_aligned(0x80000000, 1).is_err());
        assert!(vm.allocate(None, PAGE_SIZE).is_ok());
    }

    #[test]
    fn freeing_between_two_free_chunks_restores_the_full_heap() {
        let mut vm = VMAllocator::new(PAGE_SIZE, PAGE_SIZE);
        let mut heap = HeapAllocator::new(&mut vm, PAGE_SIZE);
        let a = heap.alloc(&mut vm, 16).unwrap();
        let b = heap.alloc(&mut vm, 16).unwrap();
        let c = heap.alloc(&mut vm, 16).unwrap();
        assert_eq!(heap.free(&mut vm, a.base), 16);
        assert_eq!(heap.free(&mut vm, c.base), 16);
        assert_eq!(heap.free(&mut vm, b.base), 16);
        let whole = heap.alloc(&mut vm, PAGE_SIZE).unwrap();
        assert_eq!(whole.base, PAGE_SIZE);
        assert_eq!(whole.size.get(), PAGE_SIZE);
    }

    #[test]
    fn small_aligned_allocations_preserve_padding_without_using_a_page_each() {
        let mut vm = VMAllocator::new(PAGE_SIZE, PAGE_SIZE);
        let mut heap = HeapAllocator::new(&mut vm, PAGE_SIZE);
        let first = heap.alloc(&mut vm, 16).unwrap();
        let aligned = heap.memalign(&mut vm, 64, 17).unwrap();
        assert!(aligned.base.is_multiple_of(64));
        assert_eq!(aligned.size.get(), 32);
        assert!(!heap.is_vm_allocation(aligned.base));
        let padding = heap.alloc(&mut vm, 48).unwrap();
        assert_eq!(padding.base, first.base + 16);
        assert_eq!(heap.free(&mut vm, padding.base), 48);
        assert_eq!(heap.free(&mut vm, aligned.base), 32);
        assert_eq!(heap.free(&mut vm, first.base), 16);
        assert_eq!(heap.alloc(&mut vm, PAGE_SIZE).unwrap().base, PAGE_SIZE);
    }

    #[test]
    fn interleaved_aligned_allocations_never_overlap_and_release_all_padding() {
        let mut vm = VMAllocator::new(PAGE_SIZE, PAGE_SIZE * 2);
        let mut heap = HeapAllocator::new(&mut vm, PAGE_SIZE * 2);
        let mut active: Vec<Chunk> = Vec::new();
        let mut seed = 7u32;
        for step in 0..500 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if step % 3 == 0 && !active.is_empty() {
                let old = active.swap_remove(seed as usize % active.len());
                assert_eq!(heap.free(&mut vm, old.base), old.size.get());
                assert_eq!(heap.allocated_size(old.base), None);
            } else {
                let alignment = [32, 64, 256, 4096][(seed as usize >> 8) % 4];
                let size = [1, 17, 129, 511][(seed as usize >> 16) % 4];
                if let Some(alloc) = heap.memalign(&mut vm, alignment, size) {
                    assert!(alloc.base.is_multiple_of(alignment));
                    assert!(alloc.size.get() >= size);
                    for other in &active {
                        assert!(alloc.last_byte() < other.base || other.last_byte() < alloc.base);
                    }
                    active.push(alloc);
                }
            }
        }
        for old in active {
            assert_eq!(heap.free(&mut vm, old.base), old.size.get());
        }
        assert_eq!(heap.alloc(&mut vm, PAGE_SIZE * 2).unwrap().base, PAGE_SIZE);
    }
}
