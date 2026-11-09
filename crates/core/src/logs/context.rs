//! Lines around each match of a filter, taken from the match's own source.
//!
//! A tail interleaves many containers, so the lines stored next to a match mostly come
//! from other pods. The lines that explain a match are the ones its own container wrote
//! before and after it, so context follows the source.
//!
//! Each match shows as a block: up to `lines` lines of its source before it, the match,
//! and up to `lines` after it. When the next match comes from the same source with no
//! more than `2 * lines` of its lines between them, the two blocks join, so no line shows
//! twice and no separator splits them. Context looks at most `REACH` stored lines either
//! side of a match, which bounds the work for a source that writes rarely.

use std::io;
use std::ops::Range;

use super::filter::LogFilterScan;
use super::store::LogLine;

/// How many stored lines either side of a match context looks through.
pub(super) const REACH: u64 = 4096;
/// Stored lines read at a time while looking for context.
const STEP: u64 = 512;

/// A line in a block, and whether the filter matches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextLine {
    pub line: LogLine,
    pub matched: bool,
}

/// A match with the lines around it from its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBlock {
    /// Whether lines were skipped since the block before, so a separator shows.
    pub separated: bool,
    pub lines: Vec<ContextLine>,
    /// Where the match is in `lines`.
    pub matched_at: usize,
}

/// A stored line's sequence number and source.
pub(super) type Header = (u64, u16);

/// The sequence numbers a block shows around its match.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct BlockSeqs {
    pub separated: bool,
    pub before: Vec<u64>,
    pub after: Vec<u64>,
}

/// Works out the block of the match `at`, from the matches either side of it. `fetch`
/// returns the headers of the held lines in a range, oldest first.
pub(super) fn block_seqs(
    previous: Option<Header>,
    at: Header,
    next: Option<Header>,
    lines: usize,
    held: Range<u64>,
    fetch: &mut impl FnMut(Range<u64>) -> io::Result<Vec<Header>>,
) -> io::Result<BlockSeqs> {
    let (seq, source) = at;
    let mut between = |from: u64, to: u64| -> io::Result<Option<Vec<u64>>> {
        if to - from > REACH {
            return Ok(None);
        }
        let seqs: Vec<u64> = fetch(from + 1..to)?
            .into_iter()
            .filter(|header| header.1 == source)
            .map(|header| header.0)
            .collect();
        Ok((seqs.len() <= 2 * lines).then_some(seqs))
    };
    let joined_before = match previous {
        Some((before, from)) if from == source => between(before, seq)?.is_some(),
        _ => false,
    };
    let joined_after = match next {
        Some((after, from)) if from == source => between(seq, after)?,
        _ => None,
    };
    let before = if joined_before {
        Vec::new()
    } else {
        let floor = held.start.max(seq.saturating_sub(REACH));
        let mut found = Vec::new();
        let mut end = seq;
        while found.len() < lines && end > floor {
            let start = end.saturating_sub(STEP).max(floor);
            let mut window: Vec<u64> = fetch(start..end)?
                .into_iter()
                .filter(|header| header.1 == source)
                .map(|header| header.0)
                .collect();
            window.append(&mut found);
            found = window;
            end = start;
        }
        found.split_off(found.len().saturating_sub(lines))
    };
    let after = match joined_after {
        Some(between) => between,
        None => {
            let ceiling = held.end.min(seq + 1 + REACH);
            let mut found = Vec::new();
            let mut start = seq + 1;
            while found.len() < lines && start < ceiling {
                let end = (start + STEP).min(ceiling);
                found.extend(
                    fetch(start..end)?
                        .into_iter()
                        .filter(|header| header.1 == source)
                        .map(|header| header.0),
                );
                start = end;
            }
            found.truncate(lines);
            found
        }
    };
    Ok(BlockSeqs {
        separated: previous.is_some() && !joined_before,
        before,
        after,
    })
}

impl LogFilterScan {
    /// The match at `position` with up to `lines` lines of its source around it, or
    /// `None` past the last match.
    pub fn context(&self, position: usize, lines: usize) -> io::Result<Option<ContextBlock>> {
        let first = position.saturating_sub(1);
        let seqs = self
            .found
            .lock()
            .expect("filter lock")
            .matches
            .select(first..position + 2);
        let store = &self.reader.store;
        let mut headers = Vec::with_capacity(seqs.len());
        store
            .lock()
            .expect("log store lock")
            .each_seq(&seqs, |seq, record| headers.push((seq, record.source)))?;
        let header = |seq: Option<&u64>| {
            let seq = *seq?;
            headers.iter().find(|header| header.0 == seq).copied()
        };
        let (previous, at, next) = if position == 0 {
            (None, header(seqs.first()), header(seqs.get(1)))
        } else {
            (
                header(seqs.first()),
                header(seqs.get(1)),
                header(seqs.get(2)),
            )
        };
        let Some(at) = at else {
            return Ok(None);
        };
        let held = store.lock().expect("log store lock").range();
        let block = block_seqs(previous, at, next, lines, held, &mut |range| {
            let mut out = Vec::new();
            store
                .lock()
                .expect("log store lock")
                .sources(range, &mut out)?;
            Ok(out)
        })?;
        let mut wanted = block.before;
        wanted.push(at.0);
        wanted.extend(block.after);
        let read = store.lock().expect("log store lock").read_seqs(&wanted)?;
        let found = self.found.lock().expect("filter lock");
        let lines: Vec<ContextLine> = read
            .into_iter()
            .map(|line| ContextLine {
                matched: line.seq == at.0 || found.matches.contains(line.seq),
                line,
            })
            .collect();
        let matched_at = lines
            .iter()
            .position(|line| line.line.seq == at.0)
            .unwrap_or_default();
        Ok(Some(ContextBlock {
            separated: block.separated,
            lines,
            matched_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headers for a stored sequence of sources, one per line from 0.
    fn fetcher(sources: &[u16]) -> impl FnMut(Range<u64>) -> io::Result<Vec<Header>> + '_ {
        move |range: Range<u64>| {
            Ok(range
                .filter(|seq| (*seq as usize) < sources.len())
                .map(|seq| (seq, sources[seq as usize]))
                .collect())
        }
    }

    fn block(
        sources: &[u16],
        previous: Option<u64>,
        at: u64,
        next: Option<u64>,
        lines: usize,
    ) -> BlockSeqs {
        let header = |seq: u64| (seq, sources[seq as usize]);
        block_seqs(
            previous.map(header),
            header(at),
            next.map(header),
            lines,
            0..sources.len() as u64,
            &mut fetcher(sources),
        )
        .unwrap()
    }

    #[test]
    fn context_comes_from_the_matchs_own_source() {
        // Two sources interleave: 0 on even lines, 1 on odd lines.
        let sources: Vec<u16> = (0..40).map(|seq| (seq % 2) as u16).collect();

        let found = block(&sources, None, 20, None, 3);

        assert_eq!(
            found,
            BlockSeqs {
                separated: false,
                before: vec![14, 16, 18],
                after: vec![22, 24, 26],
            }
        );
    }

    #[test]
    fn context_stops_at_the_ends_of_the_store() {
        let sources = vec![0u16; 5];

        let found = block(&sources, None, 1, None, 3);

        assert_eq!(found.before, [0]);
        assert_eq!(found.after, [2, 3, 4]);
    }

    #[test]
    fn close_matches_from_one_source_join() {
        let sources = vec![0u16; 40];

        let first = block(&sources, None, 10, Some(15), 3);
        let second = block(&sources, Some(10), 15, Some(30), 3);
        let third = block(&sources, Some(15), 30, None, 3);

        assert_eq!(first.after, [11, 12, 13, 14], "every line between joins");
        assert!(!second.separated);
        assert!(second.before.is_empty());
        assert_eq!(second.after, [16, 17, 18]);
        assert!(third.separated, "too far apart to join");
        assert_eq!(third.before, [27, 28, 29]);
    }

    #[test]
    fn matches_from_other_sources_never_join() {
        let sources: Vec<u16> = (0..20).map(|seq| (seq % 2) as u16).collect();

        let found = block(&sources, Some(9), 10, Some(11), 2);

        assert!(found.separated);
        assert_eq!(found.before, [6, 8]);
        assert_eq!(found.after, [12, 14]);
    }

    #[test]
    fn a_quiet_source_finds_context_only_within_reach() {
        let mut sources = vec![1u16; (3 * REACH) as usize];
        let at = 2 * REACH;
        sources[at as usize] = 0;
        sources[(at - REACH - 5) as usize] = 0;
        sources[(at - 5) as usize] = 0;

        let found = block(&sources, None, at, None, 3);

        assert_eq!(found.before, [at - 5]);
        assert!(found.after.is_empty());
    }
}
