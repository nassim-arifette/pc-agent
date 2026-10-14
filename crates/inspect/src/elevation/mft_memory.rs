//! Compact storage with exact allocation accounting for the local MFT index and run list.
//!
//! Items live in chunks that double from a small first chunk up to a fixed size. A small bound
//! therefore wastes little, and a large index never reallocates: there is no transient copy of a
//! big array and every allocated byte is visible to `bytes()`. Chunk pointers are reserved once
//! for the bound, so pushing never grows a second array either.
use std::mem::size_of;

/// Index of the chunk holding `index`, and the offset inside it. Chunk `k` holds
/// `first << min(k, g)` items, where `first << g == last`.
fn locate(index: usize, first: usize, last: usize) -> (usize, usize) {
    let growing = (last / first).trailing_zeros() as usize;
    let small = first * ((2 << growing) - 1);
    if index < small {
        let k = (usize::BITS - 1 - (index / first + 1).leading_zeros()) as usize;
        (k, index - first * ((1 << k) - 1))
    } else {
        let rest = index - small;
        (growing + 1 + rest / last, rest % last)
    }
}

fn chunk_len(k: usize, first: usize, last: usize) -> usize {
    first << k.min((last / first).trailing_zeros() as usize)
}

/// Number of chunks needed to hold `items` items.
fn chunks_for(items: usize, first: usize, last: usize) -> usize {
    if items == 0 {
        0
    } else {
        locate(items - 1, first, last).0 + 1
    }
}

/// Fixed-size items in doubling-then-fixed chunks. `LAST / FIRST` must be a power of two.
pub(super) struct Chunked<T, const FIRST: usize, const LAST: usize> {
    chunks: Vec<Vec<T>>,
    len: usize,
}

impl<T: Copy, const FIRST: usize, const LAST: usize> Chunked<T, FIRST, LAST> {
    /// Reserves the chunk pointers needed to hold `bound_bytes` of items; nothing else.
    pub(super) fn with_bound(bound_bytes: usize) -> Self {
        let mut chunks = Vec::new();
        let _ = chunks.try_reserve_exact(chunks_for(bound_bytes / size_of::<T>(), FIRST, LAST));
        Self { chunks, len: 0 }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Allocated bytes: chunk capacities plus the reserved pointer array.
    pub(super) fn bytes(&self) -> usize {
        self.chunks
            .iter()
            .map(|chunk| chunk.capacity() * size_of::<T>())
            .sum::<usize>()
            + self.chunks.capacity() * size_of::<Vec<T>>()
    }

    /// Bytes that pushing `additional` more items would allocate; `usize::MAX` beyond the bound.
    pub(super) fn growth(&self, additional: usize) -> usize {
        let needed = chunks_for(self.len + additional, FIRST, LAST);
        if needed > self.chunks.capacity() {
            return usize::MAX;
        }
        (self.chunks.len()..needed)
            .map(|k| chunk_len(k, FIRST, LAST) * size_of::<T>())
            .sum()
    }

    /// Appends one item; false (and nothing changed) beyond the bound or if its chunk cannot be
    /// allocated.
    pub(super) fn push(&mut self, value: T) -> bool {
        let (k, _) = locate(self.len, FIRST, LAST);
        if k == self.chunks.len() {
            let mut chunk = Vec::new();
            if k == self.chunks.capacity()
                || chunk.try_reserve_exact(chunk_len(k, FIRST, LAST)).is_err()
            {
                return false;
            }
            self.chunks.push(chunk);
        }
        self.chunks[k].push(value);
        self.len += 1;
        true
    }

    /// Reserved chunk slots: an upper bound of the chunk count.
    pub(super) fn chunk_slots(&self) -> usize {
        self.chunks.capacity()
    }

    pub(super) fn get(&self, index: usize) -> T {
        let (k, offset) = locate(index, FIRST, LAST);
        self.chunks[k][offset]
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flatten()
    }

    /// Mutable chunks, e.g. to sort each one in place without another allocation.
    pub(super) fn chunks_mut(&mut self) -> &mut [Vec<T>] {
        &mut self.chunks
    }
}

/// Filenames as UTF-8, back to back in chunks. A name never straddles two chunks; the unused
/// tail of a chunk is at most one name long (765 bytes for 255 UTF-16 units).
pub(super) struct Arena {
    chunks: Vec<Vec<u8>>,
    end: usize,
}

const ARENA_FIRST: usize = 4096;
const ARENA_LAST: usize = 1 << 20;

impl Arena {
    pub(super) fn with_bound(bound_bytes: usize) -> Self {
        let mut chunks = Vec::new();
        let _ = chunks.try_reserve_exact(chunks_for(bound_bytes, ARENA_FIRST, ARENA_LAST) + 1);
        Self { chunks, end: 0 }
    }

    pub(super) fn bytes(&self) -> usize {
        self.chunks.iter().map(Vec::capacity).sum::<usize>()
            + self.chunks.capacity() * size_of::<Vec<u8>>()
    }

    /// Where a name of `len` bytes starting at or after `end` is placed, and the chunk it needs.
    fn place(end: usize, len: usize) -> (usize, usize) {
        let (k, offset) = locate(end, ARENA_FIRST, ARENA_LAST);
        if offset + len <= chunk_len(k, ARENA_FIRST, ARENA_LAST) {
            (end, k)
        } else {
            (end - offset + chunk_len(k, ARENA_FIRST, ARENA_LAST), k + 1)
        }
    }

    /// Bytes that storing names of these lengths, in order, would allocate; `usize::MAX` beyond
    /// the bound.
    pub(super) fn growth(&self, lengths: impl IntoIterator<Item = usize>) -> usize {
        let (mut end, mut chunks, mut bytes) = (self.end, self.chunks.len(), 0);
        for len in lengths {
            let (at, k) = Self::place(end, len);
            while chunks <= k {
                bytes += chunk_len(chunks, ARENA_FIRST, ARENA_LAST);
                chunks += 1;
            }
            end = at + len;
        }
        if chunks > self.chunks.capacity() {
            return usize::MAX;
        }
        bytes
    }

    /// Stores a name (at most `ARENA_FIRST` bytes) and returns its position.
    pub(super) fn push(&mut self, name: &[u8]) -> Option<u32> {
        if name.len() > ARENA_FIRST {
            return None;
        }
        let (at, k) = Self::place(self.end, name.len());
        let position = u32::try_from(at).ok()?;
        if k >= self.chunks.capacity() {
            return None;
        }
        while self.chunks.len() <= k {
            let mut chunk = Vec::new();
            chunk
                .try_reserve_exact(chunk_len(self.chunks.len(), ARENA_FIRST, ARENA_LAST))
                .ok()?;
            self.chunks.push(chunk);
        }
        self.chunks[k].extend_from_slice(name);
        self.end = at + name.len();
        Some(position)
    }

    pub(super) fn get(&self, at: u32, len: u16) -> &[u8] {
        let (k, offset) = locate(at as usize, ARENA_FIRST, ARENA_LAST);
        &self.chunks[k][offset..offset + usize::from(len)]
    }
}

/// Conservative heap bytes of a `BTreeSet`/`BTreeMap` with `len` entries of `entry` bytes:
/// nodes hold at most 11 entries and, except the root, at least 5. Counting one node, with
/// child pointers and header, per 4 entries also covers the internal nodes.
pub(super) fn btree_bytes(len: usize, entry: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let node = 11 * entry + 12 * size_of::<usize>() + 16;
    len.div_ceil(4) * node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_positions_are_contiguous_and_doubling() {
        let mut expected = (0, 0);
        for index in 0..40_000 {
            let found = locate(index, 64, 8192);
            assert_eq!(found, expected, "index {index}");
            expected.1 += 1;
            if expected.1 == chunk_len(expected.0, 64, 8192) {
                expected = (expected.0 + 1, 0);
            }
        }
        assert_eq!(chunk_len(0, 64, 8192), 64);
        assert_eq!(chunk_len(7, 64, 8192), 8192);
        assert_eq!(chunk_len(30, 64, 8192), 8192);
    }

    #[test]
    fn growth_predicts_every_allocation_exactly() {
        let mut items = Chunked::<u64, 64, 8192>::with_bound(1 << 20);
        let reserved = items.bytes();
        for value in 0..20_000_u64 {
            let before = items.bytes();
            let growth = items.growth(1);
            assert!(items.push(value));
            assert_eq!(items.bytes(), before + growth);
        }
        assert_eq!(items.len(), 20_000);
        assert_eq!(items.get(19_999), 19_999);
        assert_eq!(items.iter().count(), 20_000);
        assert!(items.bytes() - reserved < 20_000 * 8 + 8192 * 8);

        let mut names = Arena::with_bound(4 << 20);
        let mut stored = Vec::new();
        for i in 0..5_000_usize {
            let name = "n".repeat(1 + i % 700);
            let before = names.bytes();
            let growth = names.growth([name.len()]);
            let at = names.push(name.as_bytes()).unwrap();
            assert_eq!(names.bytes(), before + growth);
            stored.push((at, name));
        }
        for (at, name) in &stored {
            assert_eq!(names.get(*at, name.len() as u16), name.as_bytes());
        }
        assert!(names.push(&[0; ARENA_FIRST + 1]).is_none());
    }
}
