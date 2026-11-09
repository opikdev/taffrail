//! The rows the log dock shows, and how to move through them.
//!
//! Every row has a [`RowId`] that sorts in the order the dock shows rows. Without context
//! it is the line's sequence number. With context, it is the sequence number of the
//! match whose block holds the row, and the row's place in that block. Ids stay put as
//! lines arrive, so a position or a selection keeps its line.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::rc::Rc;

use taffrail_core::{ContextBlock, LogFilterScan, LogLine, LogReader};

/// Where a row sits in the dock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowId {
    pub key: u64,
    pub offset: u32,
}

impl RowId {
    pub fn line(seq: u64) -> Self {
        Self {
            key: seq,
            offset: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A stored line. With context, `matched` says whether the filter matches it.
    Line {
        line: LogLine,
        matched: Option<bool>,
    },
    /// Lines of a source were skipped since the block before.
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: RowId,
    pub kind: RowKind,
}

impl Row {
    pub fn line(&self) -> Option<&LogLine> {
        match &self.kind {
            RowKind::Line { line, .. } => Some(line),
            RowKind::Separator => None,
        }
    }
}

/// Blocks of context already read, by the sequence number of their match.
#[derive(Default)]
pub struct BlockCache {
    blocks: HashMap<u64, Rc<ContextBlock>>,
}

impl BlockCache {
    /// Forgets every block, such as when lines or matches change.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

/// What the dock shows: every stored line, a filter's matches, or the matches with the
/// lines around them.
pub enum Rows<'a> {
    All {
        reader: &'a LogReader,
        held: Range<u64>,
    },
    Matches(&'a LogFilterScan),
    Context {
        scan: &'a LogFilterScan,
        lines: usize,
        cache: &'a mut BlockCache,
    },
    /// A filter that can't run.
    Nothing,
}

fn line_row(line: LogLine) -> Row {
    Row {
        id: RowId::line(line.seq),
        kind: RowKind::Line {
            line,
            matched: None,
        },
    }
}

fn block_rows(block: &ContextBlock) -> Vec<Row> {
    let key = block.lines[block.matched_at].line.seq;
    let separator = block.separated.then_some(Row {
        id: RowId { key, offset: 0 },
        kind: RowKind::Separator,
    });
    let first = u32::from(block.separated);
    separator
        .into_iter()
        .chain(block.lines.iter().enumerate().map(|(index, line)| Row {
            id: RowId {
                key,
                offset: first + index as u32,
            },
            kind: RowKind::Line {
                line: line.line.clone(),
                matched: Some(line.matched),
            },
        }))
        .collect()
}

impl Rows<'_> {
    /// The context block of the match at `position`, as rows.
    fn block(&mut self, position: usize) -> io::Result<Option<Vec<Row>>> {
        let Self::Context { scan, lines, cache } = self else {
            return Ok(None);
        };
        let Some(seq) = scan.seqs(position..position + 1).first().copied() else {
            return Ok(None);
        };
        if let Some(block) = cache.blocks.get(&seq) {
            return Ok(Some(block_rows(block)));
        }
        let Some(block) = scan.context(position, *lines)? else {
            return Ok(None);
        };
        let rows = block_rows(&block);
        cache.blocks.insert(seq, Rc::new(block));
        Ok(Some(rows))
    }

    /// How many matches, or lines without a filter, the view moves through.
    fn count(&self) -> u64 {
        match self {
            Self::All { held, .. } => held.end - held.start,
            Self::Matches(scan) | Self::Context { scan, .. } => scan.len() as u64,
            Self::Nothing => 0,
        }
    }

    /// The position among the lines or matches of the first one at or after `key`.
    fn position(&self, key: u64) -> u64 {
        match self {
            Self::All { held, .. } => key.clamp(held.start, held.end) - held.start,
            Self::Matches(scan) | Self::Context { scan, .. } => scan.position_of(key) as u64,
            Self::Nothing => 0,
        }
    }

    /// Up to `count` rows from `top` on.
    pub fn from(&mut self, top: RowId, count: usize) -> io::Result<Vec<Row>> {
        match self {
            Self::All { reader, held } => {
                let start = top.key.clamp(held.start, held.end);
                let end = (start + count as u64).min(held.end);
                Ok(reader.read(start..end)?.into_iter().map(line_row).collect())
            }
            Self::Matches(scan) => {
                let position = scan.position_of(top.key);
                Ok(scan
                    .lines(position..position + count)?
                    .into_iter()
                    .map(line_row)
                    .collect())
            }
            Self::Context { scan, .. } => {
                let mut position = scan.position_of(top.key);
                let mut rows = Vec::with_capacity(count);
                while rows.len() < count {
                    let Some(block) = self.block(position)? else {
                        break;
                    };
                    rows.extend(block.into_iter().filter(|row| row.id >= top));
                    position += 1;
                }
                rows.truncate(count);
                Ok(rows)
            }
            Self::Nothing => Ok(Vec::new()),
        }
    }

    /// The newest `count` rows.
    pub fn last(&mut self, count: usize) -> io::Result<Vec<Row>> {
        match self {
            Self::All { held, .. } => {
                let start = held.end.saturating_sub(count as u64).max(held.start);
                self.from(RowId::line(start), count)
            }
            Self::Matches(scan) => {
                let start = scan.len().saturating_sub(count);
                Ok(scan
                    .lines(start..start + count)?
                    .into_iter()
                    .map(line_row)
                    .collect())
            }
            Self::Context { scan, .. } => {
                let mut position = scan.len();
                let mut rows: Vec<Row> = Vec::new();
                while rows.len() < count && position > 0 {
                    position -= 1;
                    let Some(mut block) = self.block(position)? else {
                        break;
                    };
                    block.append(&mut rows);
                    rows = block;
                }
                Ok(rows.split_off(rows.len().saturating_sub(count)))
            }
            Self::Nothing => Ok(Vec::new()),
        }
    }

    /// `count` rows from `top`, moved back to show a full view when too few rows follow it.
    pub fn view(&mut self, top: RowId, count: usize) -> io::Result<Vec<Row>> {
        let rows = self.from(top, count)?;
        if rows.len() >= count {
            return Ok(rows);
        }
        let last = self.last(count)?;
        Ok(if last.len() > rows.len() { last } else { rows })
    }

    /// The row `delta` rows from `id`, stopping at the first and last rows.
    pub fn step(&mut self, id: RowId, delta: i64) -> io::Result<RowId> {
        let count = self.count();
        if count == 0 {
            return Ok(id);
        }
        match self {
            Self::All { held, .. } => Ok(RowId::line(
                id.key
                    .clamp(held.start, held.end - 1)
                    .saturating_add_signed(delta)
                    .clamp(held.start, held.end - 1),
            )),
            Self::Matches(scan) => {
                let position = (scan.position_of(id.key) as u64)
                    .saturating_add_signed(delta)
                    .min(count - 1) as usize;
                Ok(scan
                    .seqs(position..position + 1)
                    .first()
                    .map_or(id, |seq| RowId::line(*seq)))
            }
            Self::Context { .. } => self.step_context(id, delta),
            Self::Nothing => Ok(id),
        }
    }

    fn step_context(&mut self, id: RowId, delta: i64) -> io::Result<RowId> {
        let count = self.count();
        let mut position = self.position(id.key).min(count - 1) as usize;
        let Some(mut block) = self.block(position)? else {
            return Ok(id);
        };
        let mut index = block.iter().position(|row| row.id >= id).unwrap_or(0) as i64 + delta;
        while index < 0 {
            if position == 0 {
                return Ok(block[0].id);
            }
            position -= 1;
            let Some(previous) = self.block(position)? else {
                return Ok(block[0].id);
            };
            index += previous.len() as i64;
            block = previous;
        }
        while index as usize >= block.len() {
            if position as u64 + 1 >= count {
                return Ok(block[block.len() - 1].id);
            }
            index -= block.len() as i64;
            position += 1;
            let Some(next) = self.block(position)? else {
                return Ok(block[block.len() - 1].id);
            };
            block = next;
        }
        Ok(block[index as usize].id)
    }

    /// The rows from `first` to `last`, both included, stopping after `limit`.
    pub fn between(&mut self, first: RowId, last: RowId, limit: usize) -> io::Result<Vec<Row>> {
        let mut rows = Vec::new();
        let mut top = first;
        while rows.len() < limit {
            let chunk = self.from(top, (limit - rows.len()).min(4096))?;
            let Some(end) = chunk.last().map(|row| row.id) else {
                break;
            };
            let before = rows.len();
            rows.extend(chunk.into_iter().filter(|row| row.id <= last));
            if end >= last || rows.len() == before {
                break;
            }
            let next = self.step(end, 1)?;
            if next == end {
                break;
            }
            top = next;
        }
        Ok(rows)
    }

    /// The row that shows line `seq`: in a context block, the match's own row.
    pub fn row_of(&mut self, seq: u64) -> io::Result<RowId> {
        if !matches!(self, Self::Context { .. }) {
            return Ok(RowId::line(seq));
        }
        let position = self.position(seq) as usize;
        let rows = self.block(position)?.unwrap_or_default();
        Ok(rows
            .iter()
            .find(|row| row.line().is_some_and(|line| line.seq == seq))
            .map_or(RowId::line(seq), |row| row.id))
    }
}
