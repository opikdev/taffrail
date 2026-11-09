//! A sorted set of line sequence numbers, kept compact enough to hold a match for every
//! stored line.
//!
//! Lines are grouped in chunks of 65,536 sequence numbers. A chunk with few matches keeps
//! them as a sorted list of 16-bit offsets, and a chunk with many keeps an 8 KiB bitmap, so
//! a set never takes more than about 2 bytes per match, or 1 bit per line it spans.

use std::collections::VecDeque;

const CHUNK_SHIFT: u32 = 16;
const CHUNK_LINES: u64 = 1 << CHUNK_SHIFT;
const WORDS: usize = (CHUNK_LINES / 64) as usize;
/// A list this long takes as much room as a bitmap.
const MAX_LIST: usize = WORDS * size_of::<u64>() / size_of::<u16>();

enum Lines {
    List(Vec<u16>),
    Bitmap(Box<[u64; WORDS]>),
}

struct Chunk {
    key: u64,
    count: usize,
    lines: Lines,
}

impl Chunk {
    fn new(key: u64) -> Self {
        Self {
            key,
            count: 0,
            lines: Lines::List(Vec::new()),
        }
    }

    fn insert(&mut self, offset: u16) -> bool {
        let added = match &mut self.lines {
            Lines::List(list) => match list.last() {
                Some(last) if *last < offset => {
                    list.push(offset);
                    true
                }
                None => {
                    list.push(offset);
                    true
                }
                Some(_) => match list.binary_search(&offset) {
                    Ok(_) => false,
                    Err(at) => {
                        list.insert(at, offset);
                        true
                    }
                },
            },
            Lines::Bitmap(words) => {
                let (word, bit) = (offset as usize / 64, offset % 64);
                let added = words[word] & (1 << bit) == 0;
                words[word] |= 1 << bit;
                added
            }
        };
        if added {
            self.count += 1;
            self.compact();
        }
        added
    }

    /// Turns a list that has grown as large as a bitmap into one.
    fn compact(&mut self) {
        if let Lines::List(list) = &self.lines
            && list.len() > MAX_LIST
        {
            let mut words = Box::new([0u64; WORDS]);
            for offset in list {
                words[*offset as usize / 64] |= 1 << (offset % 64);
            }
            self.lines = Lines::Bitmap(words);
        }
    }

    /// Matches in this chunk below `offset`.
    fn rank(&self, offset: u16) -> usize {
        match &self.lines {
            Lines::List(list) => list.partition_point(|at| *at < offset),
            Lines::Bitmap(words) => {
                let (word, bit) = (offset as usize / 64, offset % 64);
                let whole: u32 = words[..word].iter().map(|word| word.count_ones()).sum();
                let part = (words[word] & ((1u64 << bit) - 1)).count_ones();
                (whole + part) as usize
            }
        }
    }

    fn remove_below(&mut self, offset: u16) {
        match &mut self.lines {
            Lines::List(list) => {
                let below = list.partition_point(|at| *at < offset);
                list.drain(..below);
                self.count = list.len();
            }
            Lines::Bitmap(words) => {
                let (word, bit) = (offset as usize / 64, offset % 64);
                words[..word].fill(0);
                words[word] &= !((1u64 << bit) - 1);
                self.count = words.iter().map(|word| word.count_ones() as usize).sum();
            }
        }
    }

    /// Pushes up to `count` sequence numbers to `out`, from the `skip`th match on.
    fn select(&self, skip: usize, count: usize, out: &mut Vec<u64>) {
        let base = self.key << CHUNK_SHIFT;
        match &self.lines {
            Lines::List(list) => {
                out.extend(
                    list.iter()
                        .skip(skip)
                        .take(count)
                        .map(|offset| base + u64::from(*offset)),
                );
            }
            Lines::Bitmap(words) => {
                let mut skip = skip;
                let wanted = out.len() + count;
                for (index, word) in words.iter().enumerate() {
                    let ones = word.count_ones() as usize;
                    if skip >= ones {
                        skip -= ones;
                        continue;
                    }
                    let mut bits = *word;
                    while bits != 0 && out.len() < wanted {
                        let bit = bits.trailing_zeros();
                        bits &= bits - 1;
                        if skip > 0 {
                            skip -= 1;
                            continue;
                        }
                        out.push(base + index as u64 * 64 + u64::from(bit));
                    }
                    if out.len() == wanted {
                        return;
                    }
                }
            }
        }
    }

    fn bytes(&self) -> usize {
        size_of::<Self>()
            + match &self.lines {
                Lines::List(list) => list.capacity() * size_of::<u16>(),
                Lines::Bitmap(_) => WORDS * size_of::<u64>(),
            }
    }
}

fn split(seq: u64) -> (u64, u16) {
    (seq >> CHUNK_SHIFT, (seq & (CHUNK_LINES - 1)) as u16)
}

#[derive(Default)]
pub(super) struct MatchSet {
    chunks: VecDeque<Chunk>,
    len: usize,
}

impl MatchSet {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn insert(&mut self, seq: u64) {
        let (key, offset) = split(seq);
        let at = match self.chunks.back() {
            Some(last) if last.key == key => self.chunks.len() - 1,
            Some(last) if last.key < key => {
                self.chunks.push_back(Chunk::new(key));
                self.chunks.len() - 1
            }
            None => {
                self.chunks.push_back(Chunk::new(key));
                0
            }
            Some(_) => match self.chunks.binary_search_by_key(&key, |chunk| chunk.key) {
                Ok(at) => at,
                Err(at) => {
                    self.chunks.insert(at, Chunk::new(key));
                    at
                }
            },
        };
        if self.chunks[at].insert(offset) {
            self.len += 1;
        }
    }

    /// Adds sequence numbers in ascending order. A run below or above every match already
    /// held, as a scan finds them, is added in one move.
    pub(super) fn extend(&mut self, seqs: &[u64]) {
        let mut rest = seqs;
        while let Some(&first) = rest.first() {
            let (key, _) = split(first);
            let run = rest.partition_point(|seq| split(*seq).0 == key);
            let (group, after) = rest.split_at(run);
            rest = after;
            let front = self.chunks.front().is_none_or(|chunk| chunk.key > key);
            let back = self.chunks.back().is_some_and(|chunk| chunk.key < key);
            if front || back {
                let mut chunk = Chunk::new(key);
                let offsets: Vec<u16> = group.iter().map(|seq| split(*seq).1).collect();
                chunk.count = offsets.len();
                chunk.lines = Lines::List(offsets);
                chunk.compact();
                self.len += chunk.count;
                if front {
                    self.chunks.push_front(chunk);
                } else {
                    self.chunks.push_back(chunk);
                }
                continue;
            }
            for seq in group {
                self.insert(*seq);
            }
        }
    }

    /// Forgets every match below `seq`, such as lines the store has dropped.
    pub(super) fn remove_below(&mut self, seq: u64) {
        let (key, offset) = split(seq);
        while self.chunks.front().is_some_and(|chunk| chunk.key < key) {
            let chunk = self.chunks.pop_front().expect("checked above");
            self.len -= chunk.count;
        }
        if let Some(chunk) = self.chunks.front_mut()
            && chunk.key == key
        {
            let before = chunk.count;
            chunk.remove_below(offset);
            self.len -= before - chunk.count;
            if chunk.count == 0 {
                self.chunks.pop_front();
            }
        }
    }

    /// How many matches come before `seq`.
    pub(super) fn rank(&self, seq: u64) -> usize {
        let (key, offset) = split(seq);
        let mut rank = 0;
        for chunk in &self.chunks {
            if chunk.key > key {
                break;
            }
            if chunk.key == key {
                return rank + chunk.rank(offset);
            }
            rank += chunk.count;
        }
        rank
    }

    /// The sequence numbers of matches `positions`, in order.
    pub(super) fn select(&self, positions: std::ops::Range<usize>) -> Vec<u64> {
        let mut out = Vec::with_capacity(positions.len().min(self.len));
        let mut skip = positions.start;
        for chunk in &self.chunks {
            if out.len() == positions.len() {
                break;
            }
            if skip >= chunk.count {
                skip -= chunk.count;
                continue;
            }
            chunk.select(skip, positions.len() - out.len(), &mut out);
            skip = 0;
        }
        out
    }

    /// Roughly how much memory the set takes.
    pub(super) fn bytes(&self) -> usize {
        size_of::<Self>() + self.chunks.iter().map(Chunk::bytes).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(seqs: impl IntoIterator<Item = u64>) -> MatchSet {
        let mut set = MatchSet::default();
        for seq in seqs {
            set.insert(seq);
        }
        set
    }

    #[test]
    fn keeps_numbers_sorted_whatever_order_they_arrive_in() {
        let set = set([500_000, 3, 70_000, 1, 3, 2]);

        assert_eq!(set.len(), 5);
        assert_eq!(set.select(0..10), [1, 2, 3, 70_000, 500_000]);
        assert_eq!(set.select(2..4), [3, 70_000]);
        assert!(set.select(5..9).is_empty());
    }

    #[test]
    fn extending_below_above_and_inside_keeps_one_order() {
        let mut set = set([200_000, 200_005]);

        set.extend(&[1, 2, 70_000]);
        set.extend(&[300_000, 300_001]);
        set.extend(&[3, 200_001, 200_005]);

        assert_eq!(set.len(), 9);
        assert_eq!(
            set.select(0..20),
            [1, 2, 3, 70_000, 200_000, 200_001, 200_005, 300_000, 300_001]
        );
    }

    #[test]
    fn ranks_count_the_matches_before_a_number() {
        let set = set([10, 20, 70_000, 140_000]);

        assert_eq!(set.rank(0), 0);
        assert_eq!(set.rank(10), 0);
        assert_eq!(set.rank(11), 1);
        assert_eq!(set.rank(69_999), 2);
        assert_eq!(set.rank(70_001), 3);
        assert_eq!(set.rank(u64::MAX), 4);
    }

    #[test]
    fn a_dense_chunk_turns_into_a_bitmap_and_stays_correct() {
        let seqs: Vec<u64> = (0..CHUNK_LINES).step_by(3).collect();
        let set = set(seqs.iter().rev().copied());

        assert!(matches!(set.chunks[0].lines, Lines::Bitmap(_)));
        assert_eq!(set.len(), seqs.len());
        assert_eq!(set.select(0..seqs.len()), seqs);
        assert_eq!(set.select(1_000..1_003), seqs[1_000..1_003]);
        assert_eq!(set.rank(3_001), 1_001);
        assert!(set.bytes() < 9 * 1024);
    }

    #[test]
    fn removing_below_a_number_drops_whole_and_partial_chunks() {
        for dense in [false, true] {
            let step = if dense { 2 } else { 1_000 };
            let mut set = set((0..3 * CHUNK_LINES).step_by(step));
            let before = set.len();

            set.remove_below(CHUNK_LINES + 10);

            let first = set.select(0..1)[0];
            assert!(first >= CHUNK_LINES + 10 && first < CHUNK_LINES + 10 + step as u64);
            assert_eq!(
                set.len(),
                before - set_len_below(CHUNK_LINES + 10, step as u64)
            );
            assert_eq!(set.rank(first), 0);
        }
    }

    fn set_len_below(seq: u64, step: u64) -> usize {
        seq.div_ceil(step) as usize
    }

    #[test]
    fn removing_everything_leaves_an_empty_set() {
        let mut set = set([1, 2, 3]);

        set.remove_below(4);

        assert_eq!(set.len(), 0);
        assert!(set.chunks.is_empty());
        assert!(set.select(0..5).is_empty());
    }

    #[test]
    fn a_match_for_every_line_takes_about_a_bit_per_line() {
        let lines = 10 * CHUNK_LINES;
        let set = set(0..lines);

        assert_eq!(set.len() as u64, lines);
        assert!(set.bytes() as u64 <= lines / 8 + 1024);
    }
}
