use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;

use super::parse::{Severity, line_severity, split_timestamp};

const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SEGMENTS: usize = 16;
const INDEX_STRIDE: u64 = 256;
const HEADER_BYTES: usize = size_of::<u16>() + size_of::<u8>() + size_of::<u32>();

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
    pub text: Vec<u8>,
}

impl Record {
    /// A line from a container, with its severity found.
    pub fn new(source: u16, text: Vec<u8>) -> Self {
        Self {
            source,
            severity: line_severity(&text),
            text,
        }
    }

    /// A line Taffrail writes itself, such as why a stream ended.
    pub fn note(source: u16, text: Vec<u8>) -> Self {
        Self {
            source,
            severity: Severity::Unknown,
            text,
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

    fn read(&self, range: Range<u64>, lines: &mut Vec<LogLine>) -> io::Result<()> {
        let first_block = (range.start - self.first_seq) / INDEX_STRIDE;
        let end_block = (range.end - self.first_seq).div_ceil(INDEX_STRIDE);
        let start = self.index[first_block as usize];
        let end = self
            .index
            .get(end_block as usize)
            .copied()
            .unwrap_or(self.bytes);
        let mut encoded = vec![0; (end - start) as usize];
        (&self.file).seek(SeekFrom::Start(start))?;
        (&self.file).read_exact(&mut encoded)?;

        let mut seq = self.first_seq + first_block * INDEX_STRIDE;
        let mut rest = encoded.as_slice();
        while seq < range.end {
            let (source, severity, text, remaining) = decode(rest)?;
            if seq >= range.start {
                lines.push(LogLine {
                    seq,
                    source,
                    severity,
                    timestamp: split_timestamp(text).0,
                    text: String::from_utf8_lossy(text).into_owned(),
                });
            }
            rest = remaining;
            seq += 1;
        }
        Ok(())
    }
}

fn encode(record: &Record, encoded: &mut Vec<u8>) {
    encoded.extend_from_slice(&record.source.to_le_bytes());
    encoded.push(record.severity.to_byte());
    encoded.extend_from_slice(&(record.text.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&record.text);
}

fn decode(encoded: &[u8]) -> io::Result<(u16, Severity, &[u8], &[u8])> {
    let truncated = || io::Error::new(io::ErrorKind::InvalidData, "Truncated log record");
    let (header, rest) = encoded
        .split_at_checked(HEADER_BYTES)
        .ok_or_else(truncated)?;
    let source = u16::from_le_bytes([header[0], header[1]]);
    let severity = Severity::from_byte(header[2]);
    let length = u32::from_le_bytes([header[3], header[4], header[5], header[6]]);
    let (text, rest) = rest
        .split_at_checked(length as usize)
        .ok_or_else(truncated)?;
    Ok((source, severity, text, rest))
}

/// Log lines kept on disk in unnamed temporary files, which the OS removes on close.
///
/// Memory use is bounded by the sparse index, and disk use by dropping the oldest segment.
pub(super) struct LogStore {
    segments: VecDeque<Segment>,
    segment_bytes: u64,
    max_segments: usize,
}

impl LogStore {
    pub(super) fn new() -> io::Result<Self> {
        Self::with_limits(SEGMENT_BYTES, MAX_SEGMENTS)
    }

    fn with_limits(segment_bytes: u64, max_segments: usize) -> io::Result<Self> {
        Ok(Self {
            segments: VecDeque::from([Segment::new(0)?]),
            segment_bytes,
            max_segments,
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
        self.current().write(&encoded)
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
                    text: "2026-11-07T10:42:06.5Z ERROR boom".into()
                },
                LogLine {
                    seq: 1,
                    source: 1,
                    severity: Severity::Warning,
                    timestamp: None,
                    text: r#"{"level":"warn"}"#.into()
                },
                LogLine {
                    seq: 2,
                    source: 2,
                    severity: Severity::Unknown,
                    timestamp: None,
                    text: "[taffrail: ERROR tail ended]".into()
                },
            ]
        );
        assert_eq!(read[0].message(), "ERROR boom");
        assert_eq!(read[1].message(), r#"{"level":"warn"}"#);
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
