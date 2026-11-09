use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;

use k8s_openapi::jiff::Timestamp;
use tokio::sync::watch;

use super::parse::{Severity, line_severity, parse_rfc3339, split_timestamp};

pub(super) const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_SEGMENTS: usize = 16;
const INDEX_STRIDE: u64 = 256;
const HEADER_BYTES: usize = size_of::<u16>() + size_of::<u8>() + size_of::<u32>();
/// Set in the severity byte of a record that says lines may be missing.
const GAP_FLAG: u8 = 0x80;
const GAP_PREFIX: &str = "[taffrail: lines may be missing from ";
const GAP_SEPARATOR: &str = " to ";

/// A time range in which a source's lines may be missing, in nanoseconds since the
/// Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub from: i64,
    pub to: i64,
}

impl Gap {
    fn text(self) -> Vec<u8> {
        format!(
            "{GAP_PREFIX}{}{GAP_SEPARATOR}{}]",
            rfc3339(self.from),
            rfc3339(self.to)
        )
        .into_bytes()
    }

    fn parse(text: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(text).ok()?;
        let range = text.strip_prefix(GAP_PREFIX)?.strip_suffix(']')?;
        let (from, to) = range.split_once(GAP_SEPARATOR)?;
        Some(Self {
            from: parse_rfc3339(from.as_bytes())?,
            to: parse_rfc3339(to.as_bytes())?,
        })
    }
}

fn rfc3339(unix_nanos: i64) -> String {
    Timestamp::from_nanosecond(i128::from(unix_nanos))
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_string()
}

/// One log line and the position it arrived in across all sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub seq: u64,
    pub source: u16,
    /// Found when the line was stored.
    pub severity: Severity,
    /// Nanoseconds since the Unix epoch, from the Kubernetes timestamp before the text.
    pub timestamp: Option<i64>,
    /// The line as stored, with its Kubernetes timestamp.
    pub text: String,
    /// Set on the line Taffrail writes when this source's lines may be missing.
    pub gap: Option<Gap>,
}

impl LogLine {
    /// The line as the container wrote it, without its Kubernetes timestamp.
    pub fn message(&self) -> &str {
        match self.timestamp {
            Some(_) => self.text.split_once(' ').map_or("", |(_, message)| message),
            None => &self.text,
        }
    }
}

/// A line on its way into the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub source: u16,
    pub severity: Severity,
    pub gap: bool,
    pub text: Vec<u8>,
}

impl Record {
    /// A line from a container, with its severity found.
    pub fn new(source: u16, text: Vec<u8>) -> Self {
        Self {
            source,
            severity: line_severity(&text),
            gap: false,
            text,
        }
    }

    /// A line Taffrail writes itself, such as why a stream ended.
    pub fn note(source: u16, text: Vec<u8>) -> Self {
        Self {
            source,
            severity: Severity::Unknown,
            gap: false,
            text,
        }
    }

    /// A line Taffrail writes when lines from `source` may be missing.
    pub fn gap(source: u16, gap: Gap) -> Self {
        Self {
            source,
            severity: Severity::Unknown,
            gap: true,
            text: gap.text(),
        }
    }
}

struct Segment {
    file: File,
    bytes: u64,
    first_seq: u64,
    lines: u64,
    index: Vec<u64>,
}

impl Segment {
    fn new(first_seq: u64) -> io::Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            bytes: 0,
            first_seq,
            lines: 0,
            index: Vec::new(),
        })
    }

    fn end_seq(&self) -> u64 {
        self.first_seq + self.lines
    }

    fn write(&mut self, encoded: &[u8]) -> io::Result<()> {
        (&self.file).seek(SeekFrom::Start(self.bytes))?;
        (&self.file).write_all(encoded)?;
        self.bytes += encoded.len() as u64;
        Ok(())
    }

    /// Reads the records of the index blocks that hold `range` into `encoded`, and returns
    /// the sequence number of the first record read.
    fn encoded(&self, range: Range<u64>, encoded: &mut Vec<u8>) -> io::Result<u64> {
        let first_block = (range.start - self.first_seq) / INDEX_STRIDE;
        let end_block = (range.end - self.first_seq).div_ceil(INDEX_STRIDE);
        let start = self.index[first_block as usize];
        let end = self
            .index
            .get(end_block as usize)
            .copied()
            .unwrap_or(self.bytes);
        encoded.resize((end - start) as usize, 0);
        (&self.file).seek(SeekFrom::Start(start))?;
        (&self.file).read_exact(encoded)?;
        Ok(self.first_seq + first_block * INDEX_STRIDE)
    }

    fn read(&self, range: Range<u64>, lines: &mut Vec<LogLine>) -> io::Result<()> {
        let mut encoded = Vec::new();
        let first = self.encoded(range.clone(), &mut encoded)?;
        each_record(first, &encoded, range, |seq, record| {
            lines.push(log_line(seq, record));
        })
    }
}

fn log_line(seq: u64, record: Decoded) -> LogLine {
    LogLine {
        seq,
        source: record.source,
        severity: record.severity,
        timestamp: split_timestamp(record.text).0,
        text: String::from_utf8_lossy(record.text).into_owned(),
        gap: record.gap.then(|| Gap::parse(record.text)).flatten(),
    }
}

/// Calls `visit` with each record of `encoded` that falls in `range`, where the first
/// record of `encoded` is `seq`. It stops at the end of `range`.
pub(super) fn each_record(
    mut seq: u64,
    mut encoded: &[u8],
    range: Range<u64>,
    mut visit: impl FnMut(u64, Decoded),
) -> io::Result<()> {
    while seq < range.end {
        let (record, rest) = decode(encoded)?;
        if seq >= range.start {
            visit(seq, record);
        }
        encoded = rest;
        seq += 1;
    }
    Ok(())
}

/// Where a raw read landed: `covered` lines were read, starting from the record at `first`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Encoded {
    pub first: u64,
    pub covered: Range<u64>,
}

fn encode(record: &Record, encoded: &mut Vec<u8>) {
    encoded.extend_from_slice(&record.source.to_le_bytes());
    let gap = if record.gap { GAP_FLAG } else { 0 };
    encoded.push(record.severity.to_byte() | gap);
    encoded.extend_from_slice(&(record.text.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&record.text);
}

/// One stored record, still borrowing its text.
pub(super) struct Decoded<'a> {
    pub source: u16,
    pub severity: Severity,
    pub gap: bool,
    pub text: &'a [u8],
}

/// Decodes the first record, and returns it with the bytes after it.
fn decode(encoded: &[u8]) -> io::Result<(Decoded<'_>, &[u8])> {
    let truncated = || io::Error::new(io::ErrorKind::InvalidData, "Truncated log record");
    let (header, rest) = encoded
        .split_at_checked(HEADER_BYTES)
        .ok_or_else(truncated)?;
    let source = u16::from_le_bytes([header[0], header[1]]);
    let severity = Severity::from_byte(header[2] & !GAP_FLAG);
    let gap = header[2] & GAP_FLAG != 0;
    let length = u32::from_le_bytes([header[3], header[4], header[5], header[6]]);
    let (text, rest) = rest
        .split_at_checked(length as usize)
        .ok_or_else(truncated)?;
    Ok((
        Decoded {
            source,
            severity,
            gap,
            text,
        },
        rest,
    ))
}

/// Log lines kept on disk in unnamed temporary files, which the OS removes on close.
///
/// Memory use is bounded by the sparse index, and disk use by dropping the oldest segment.
pub(super) struct LogStore {
    segments: VecDeque<Segment>,
    segment_bytes: u64,
    max_segments: usize,
    /// The sequence numbers held, sent after every append.
    held: watch::Sender<Range<u64>>,
}

impl LogStore {
    pub(super) fn new() -> io::Result<Self> {
        Self::with_limits(SEGMENT_BYTES, MAX_SEGMENTS)
    }

    pub(super) fn with_limits(segment_bytes: u64, max_segments: usize) -> io::Result<Self> {
        Ok(Self {
            segments: VecDeque::from([Segment::new(0)?]),
            segment_bytes,
            max_segments,
            held: watch::Sender::new(0..0),
        })
    }

    pub(super) fn append(&mut self, lines: &[Record]) -> io::Result<()> {
        let mut encoded = Vec::new();
        for record in lines {
            if self.current().bytes + encoded.len() as u64 >= self.segment_bytes {
                self.current().write(&encoded)?;
                encoded.clear();
                self.rotate()?;
            }
            let segment = self.current();
            if segment.lines.is_multiple_of(INDEX_STRIDE) {
                let offset = segment.bytes + encoded.len() as u64;
                segment.index.push(offset);
            }
            segment.lines += 1;
            encode(record, &mut encoded);
        }
        let written = self.current().write(&encoded);
        self.held.send_replace(self.range());
        written
    }

    /// Changes each time lines are appended or dropped.
    pub(super) fn watch(&self) -> watch::Receiver<Range<u64>> {
        self.held.subscribe()
    }

    fn current(&mut self) -> &mut Segment {
        self.segments
            .back_mut()
            .expect("the store always has a segment")
    }

    fn rotate(&mut self) -> io::Result<()> {
        let first_seq = self.current().end_seq();
        self.segments.push_back(Segment::new(first_seq)?);
        if self.segments.len() > self.max_segments {
            self.segments.pop_front();
        }
        Ok(())
    }

    /// The sequence numbers still held, oldest first.
    pub(super) fn range(&self) -> Range<u64> {
        let first = self.segments.front().map_or(0, |segment| segment.first_seq);
        let end = self.segments.back().map_or(0, Segment::end_seq);
        first..end
    }

    /// Reads the held part of `range`, skipping any lines already dropped.
    pub(super) fn read(&self, range: Range<u64>) -> io::Result<Vec<LogLine>> {
        let held = self.range();
        let range = range.start.max(held.start)..range.end.min(held.end);
        let mut lines = Vec::new();
        for segment in &self.segments {
            let overlap = range.start.max(segment.first_seq)..range.end.min(segment.end_seq());
            if !overlap.is_empty() {
                segment.read(overlap, &mut lines)?;
            }
        }
        Ok(lines)
    }

    /// Reads the raw records of `range` into `encoded`, as far as the end of the segment
    /// that holds its start. Returns `None` once nothing of `range` is held.
    ///
    /// This only copies bytes, so a caller can decode and match them without the lock.
    pub(super) fn read_encoded(
        &self,
        range: Range<u64>,
        encoded: &mut Vec<u8>,
    ) -> io::Result<Option<Encoded>> {
        let held = self.range();
        let range = range.start.max(held.start)..range.end.min(held.end);
        if range.is_empty() {
            return Ok(None);
        }
        let segment = &self.segments[self.segment_of(range.start)];
        let covered = range.start..range.end.min(segment.end_seq());
        let first = segment.encoded(covered.clone(), encoded)?;
        Ok(Some(Encoded { first, covered }))
    }

    /// Reads the lines with these sequence numbers, which must be in ascending order.
    /// Lines already dropped are left out.
    pub(super) fn read_seqs(&self, seqs: &[u64]) -> io::Result<Vec<LogLine>> {
        let held = self.range();
        let mut lines = Vec::with_capacity(seqs.len());
        let mut encoded = Vec::new();
        let mut wanted = seqs
            .iter()
            .copied()
            .filter(|seq| held.contains(seq))
            .peekable();
        while let Some(&seq) = wanted.peek() {
            let segment = &self.segments[self.segment_of(seq)];
            let block = (seq - segment.first_seq) / INDEX_STRIDE;
            let block_end = (segment.first_seq + (block + 1) * INDEX_STRIDE).min(segment.end_seq());
            let first = segment.encoded(seq..block_end, &mut encoded)?;
            each_record(first, &encoded, seq..block_end, |at, record| {
                if wanted.next_if_eq(&at).is_some() {
                    lines.push(log_line(at, record));
                }
            })?;
            // A number out of order is skipped rather than read again.
            wanted.next_if_eq(&seq);
        }
        Ok(lines)
    }

    /// Bytes on disk, for measuring.
    #[cfg(test)]
    pub(super) fn bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.bytes).sum()
    }

    /// The index of the segment that holds `seq`, which must be held.
    fn segment_of(&self, seq: u64) -> usize {
        self.segments
            .partition_point(|segment| segment.end_seq() <= seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(range: Range<usize>) -> Vec<Record> {
        range
            .map(|index| Record::new((index % 3) as u16, format!("line {index}").into_bytes()))
            .collect()
    }

    fn texts(lines: &[LogLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn reads_back_what_was_appended() {
        let mut store = LogStore::new().unwrap();
        store
            .append(&[
                Record::new(0, b"2026-11-07T10:42:06.5Z ERROR boom".to_vec()),
                Record::new(1, b"{\"level\":\"warn\"}".to_vec()),
                Record::note(2, b"[taffrail: ERROR tail ended]".to_vec()),
            ])
            .unwrap();

        let read = store.read(0..3).unwrap();

        assert_eq!(
            read,
            [
                LogLine {
                    seq: 0,
                    source: 0,
                    severity: Severity::Error,
                    timestamp: Some(1_794_048_126_500_000_000),
                    text: "2026-11-07T10:42:06.5Z ERROR boom".into(),
                    gap: None,
                },
                LogLine {
                    seq: 1,
                    source: 1,
                    severity: Severity::Warning,
                    timestamp: None,
                    text: r#"{"level":"warn"}"#.into(),
                    gap: None,
                },
                LogLine {
                    seq: 2,
                    source: 2,
                    severity: Severity::Unknown,
                    timestamp: None,
                    text: "[taffrail: ERROR tail ended]".into(),
                    gap: None,
                },
            ]
        );
        assert_eq!(read[0].message(), "ERROR boom");
        assert_eq!(read[1].message(), r#"{"level":"warn"}"#);
    }

    #[test]
    fn a_gap_reads_back_with_its_time_range() {
        let mut store = LogStore::new().unwrap();
        let gap = Gap {
            from: 1_794_048_126_118_000_000,
            to: 1_794_048_151_000_000_000,
        };
        store
            .append(&[
                Record::gap(4, gap),
                Record::note(4, b"[taffrail: lines may be missing from x to y]".to_vec()),
            ])
            .unwrap();

        let read = store.read(0..2).unwrap();

        assert_eq!(read[0].gap, Some(gap));
        assert_eq!(read[0].source, 4);
        assert_eq!(read[0].severity, Severity::Unknown);
        assert_eq!(read[0].timestamp, None);
        assert_eq!(
            read[0].text,
            "[taffrail: lines may be missing from 2026-11-07T10:42:06.118Z to 2026-11-07T10:42:31Z]"
        );
        assert_eq!(read[1].gap, None, "only Taffrail marks a gap");
    }

    #[test]
    fn an_empty_store_reads_nothing() {
        let store = LogStore::new().unwrap();

        assert_eq!(store.range(), 0..0);
        assert_eq!(store.read(0..10).unwrap(), []);
    }

    #[test]
    fn reads_any_range_across_index_blocks() {
        let mut store = LogStore::new().unwrap();
        store.append(&lines(0..1_000)).unwrap();

        for range in [
            0..1,
            255..257,
            256..256,
            300..800,
            511..512,
            512..513,
            999..1_000,
        ] {
            let read = store.read(range.clone()).unwrap();
            let expected: Vec<String> = range.map(|index| format!("line {index}")).collect();
            assert_eq!(texts(&read), expected);
        }
    }

    #[test]
    fn appends_across_batches_keep_their_order() {
        let mut store = LogStore::new().unwrap();
        store.append(&lines(0..300)).unwrap();
        store.append(&lines(300..600)).unwrap();

        assert_eq!(
            texts(&store.read(298..302).unwrap()),
            ["line 298", "line 299", "line 300", "line 301"]
        );
    }

    #[test]
    fn a_read_past_the_end_stops_at_the_end() {
        let mut store = LogStore::new().unwrap();
        store.append(&lines(0..2)).unwrap();

        assert_eq!(texts(&store.read(1..50).unwrap()), ["line 1"]);
    }

    #[test]
    fn keeps_text_that_is_empty_or_not_utf8() {
        let mut store = LogStore::new().unwrap();
        store
            .append(&[
                Record::new(0, Vec::new()),
                Record::new(1, vec![0xff, b'a', b'\n', b'b']),
            ])
            .unwrap();

        assert_eq!(texts(&store.read(0..2).unwrap()), ["", "\u{fffd}a\nb"]);
    }

    #[test]
    fn a_full_segment_rotates_without_losing_lines() {
        let mut store = LogStore::with_limits(100, 1_000).unwrap();
        store.append(&lines(0..1_000)).unwrap();

        assert!(store.segments.len() > 2);
        assert_eq!(store.range(), 0..1_000);
        let read = store.read(0..1_000).unwrap();
        let expected: Vec<String> = (0..1_000).map(|index| format!("line {index}")).collect();
        assert_eq!(texts(&read), expected);
    }

    #[test]
    fn drops_the_oldest_lines_at_the_disk_limit() {
        let mut store = LogStore::with_limits(100, 3).unwrap();
        store.append(&lines(0..1_000)).unwrap();

        let held = store.range();

        assert_eq!(store.segments.len(), 3);
        assert_eq!(held.end, 1_000);
        assert!(held.start > 0);
        assert!(store.segments.iter().all(|segment| segment.bytes < 200));
        assert_eq!(store.read(0..held.start).unwrap(), []);
        assert_eq!(
            store.read(held.clone()).unwrap().len() as u64,
            held.end - held.start
        );
    }

    #[test]
    fn rejects_a_truncated_record() {
        assert!(decode(&[1, 0, 4, 200, 0, 0, 0, b'a']).is_err());
        assert!(decode(&[1, 0]).is_err());
    }

    /// Run with `cargo test --release -p taffrail-core store_speed -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn store_speed() {
        use std::time::Instant;

        use super::super::parse::SAMPLE_LINES;

        const LINES: usize = 1_000_000;
        let texts: Vec<Vec<u8>> = SAMPLE_LINES
            .iter()
            .cycle()
            .take(LINES)
            .map(|line| line.to_vec())
            .collect();
        let text_bytes: usize = texts.iter().map(Vec::len).sum();
        let ms = |started: Instant| started.elapsed().as_secs_f64() * 1e3;

        let started = Instant::now();
        let records: Vec<Record> = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| Record::new((index % 50) as u16, text))
            .collect();
        let find = ms(started);

        let mut store = LogStore::new().unwrap();
        let started = Instant::now();
        for batch in records.chunks(1024) {
            store.append(batch).unwrap();
        }
        let append = ms(started);
        let disk: u64 = store.segments.iter().map(|segment| segment.bytes).sum();

        let started = Instant::now();
        let read = store.read(0..LINES as u64).unwrap();
        let read_ms = ms(started);
        let started = Instant::now();
        let reparsed = read
            .iter()
            .filter(|line| line_severity(line.text.as_bytes()) == line.severity)
            .count();
        let reparse = ms(started);
        assert_eq!(reparsed, LINES);

        println!(
            "{LINES} lines, {:.0} MB of text, {:.0} MB on disk ({:.2}% for the severity byte)",
            text_bytes as f64 / 1e6,
            disk as f64 / 1e6,
            LINES as f64 / disk as f64 * 100.,
        );
        println!(
            "find severity {find:.0} ms, append {append:.0} ms, read {read_ms:.0} ms, find severity again on read {reparse:.0} ms"
        );
    }
}
