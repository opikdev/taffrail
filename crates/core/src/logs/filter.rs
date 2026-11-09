//! Filters every line a tail has stored, on a thread of its own.
//!
//! A scan checks the newest lines first and works back to the oldest, so the lines on
//! screen match first. Lines that arrive while it runs are checked before older ones.
//! Matches are kept as sequence numbers in a [`MatchSet`], never as copies of lines.

use std::io;
use std::ops::Range;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::thread;

use futures::future::{Either, select};
use regex::bytes::{Regex, RegexBuilder};
use tokio::sync::{oneshot, watch};

use super::matches::MatchSet;
use super::parse::{Severity, json_field_equals, split_timestamp};
use super::store::{Decoded, LogLine, each_record};
use super::tail::LogReader;
use super::timeline::{Mark, Timeline, TimelineView};

/// Lines read and checked at a time. Each read holds the store's lock only while it copies
/// the lines' bytes.
const SCAN_LINES: u64 = 8 * 1024;

/// What the log dock shows: lines that match every part of `query`, at `severity` or above.
///
/// The query is made of terms, which must all match:
///
/// - A word, or a `"quoted phrase"`, matches text anywhere in the line, ignoring case.
/// - `/pattern/` is a regular expression, which ignores case unless it starts with `(?-i)`.
/// - `key=value` matches a JSON line whose field `key` equals `value`, ignoring case. A
///   key such as `log.level` reaches into nested objects. Other lines match if they
///   contain `key=value`, as logfmt lines do.
/// - `source:name` matches lines from containers whose `pod/container` contains `name`.
///   A line matches if it comes from any of the sources named.
///
/// Text is matched against the line as the container wrote it, without its Kubernetes
/// timestamp. A line that marks a gap always matches, because it says lines may be missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    pub query: String,
    /// The lowest severity shown. `Severity::Unknown` shows every line, including lines with
    /// no severity.
    pub severity: Severity,
}

impl LogFilter {
    /// Whether the filter lets every line through.
    pub fn is_empty(&self) -> bool {
        self.severity == Severity::Unknown && terms(&self.query).is_empty()
    }
}

/// Why a filter can't run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    #[error("/{0}/ is not a valid regular expression")]
    Regex(String),
    #[error("/{0}/ is too large a regular expression")]
    RegexTooLarge(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    Text(String),
    Regex(String),
    Field {
        key: String,
        value: String,
        written: String,
    },
    Source(String),
}

fn terms(query: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut rest = query.trim_start();
    while !rest.is_empty() {
        let (term, after) = next_term(rest);
        terms.extend(term);
        rest = after.trim_start();
    }
    terms
}

fn next_term(input: &str) -> (Option<Term>, &str) {
    if let Some(quoted) = input.strip_prefix('"') {
        let end = quoted.find('"').unwrap_or(quoted.len());
        let phrase = &quoted[..end];
        let after = quoted.get(end + 1..).unwrap_or("");
        return (
            (!phrase.is_empty()).then(|| Term::Text(phrase.to_owned())),
            after,
        );
    }
    if let Some(pattern) = input.strip_prefix('/')
        && let Some(end) = regex_end(pattern)
    {
        let regex = pattern[..end].replace("\\/", "/");
        return (
            (!regex.is_empty()).then_some(Term::Regex(regex)),
            &pattern[end + 1..],
        );
    }
    let end = word_end(input);
    (Some(word_term(&input[..end])), &input[end..])
}

/// The closing `/` of a regular expression: the first one not escaped and followed by
/// whitespace or the end. Without one, the term is plain text, such as `/healthz`.
fn regex_end(pattern: &str) -> Option<usize> {
    let bytes = pattern.as_bytes();
    let mut escaped = false;
    for (at, byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'/' if bytes
                .get(at + 1)
                .is_none_or(|next| next.is_ascii_whitespace()) =>
            {
                return Some(at);
            }
            _ => {}
        }
    }
    None
}

/// A word ends at whitespace outside quotes, so `msg="no route"` stays one term.
fn word_end(input: &str) -> usize {
    let mut quoted = false;
    for (at, char) in input.char_indices() {
        match char {
            '"' => quoted = !quoted,
            char if char.is_whitespace() && !quoted => return at,
            _ => {}
        }
    }
    input.len()
}

fn word_term(word: &str) -> Term {
    if let Some(source) = word.strip_prefix("source:")
        && !source.is_empty()
    {
        return Term::Source(source.to_lowercase());
    }
    if let Some((key, value)) = word.split_once('=')
        && is_field_key(key)
        && !value.is_empty()
    {
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        return Term::Field {
            key: key.to_owned(),
            value: value.to_owned(),
            written: word.to_owned(),
        };
    }
    Term::Text(word.to_owned())
}

fn is_field_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'@'))
}

fn text_regex(text: &str) -> Regex {
    RegexBuilder::new(&regex::escape(text))
        .case_insensitive(true)
        .build()
        .expect("an escaped literal is a valid regular expression")
}

struct Field {
    key: Vec<u8>,
    value: String,
    /// The term as written, which a line that is not JSON must contain.
    written: Regex,
    /// Skips lines that can't hold the value, before reading their JSON.
    prefilter: Option<Regex>,
}

impl Field {
    fn matches(&self, message: &[u8]) -> bool {
        if message.trim_ascii_start().first() == Some(&b'{') {
            if self
                .prefilter
                .as_ref()
                .is_some_and(|prefilter| !prefilter.is_match(message))
            {
                return false;
            }
            if let Some(equal) = json_field_equals(message, &self.key, &self.value) {
                return equal;
            }
        }
        self.written.is_match(message)
    }
}

/// A compiled filter.
struct Matcher {
    severity: Severity,
    texts: Vec<Regex>,
    fields: Vec<Field>,
    sources: Vec<String>,
    /// Whether each source matches the `source:` terms, found on first sight.
    source_matches: Vec<Option<bool>>,
}

impl Matcher {
    fn new(filter: &LogFilter) -> Result<Self, FilterError> {
        let mut matcher = Self {
            severity: filter.severity,
            texts: Vec::new(),
            fields: Vec::new(),
            sources: Vec::new(),
            source_matches: Vec::new(),
        };
        for term in terms(&filter.query) {
            match term {
                Term::Text(text) => matcher.texts.push(text_regex(&text)),
                Term::Regex(pattern) => {
                    let regex = RegexBuilder::new(&pattern)
                        .case_insensitive(true)
                        .build()
                        .map_err(|error| match error {
                            regex::Error::CompiledTooBig(_) => FilterError::RegexTooLarge(pattern),
                            _ => FilterError::Regex(pattern),
                        })?;
                    matcher.texts.push(regex);
                }
                Term::Field {
                    key,
                    value,
                    written,
                } => {
                    let plain = value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b':')
                    });
                    matcher.fields.push(Field {
                        key: key.into_bytes(),
                        prefilter: plain.then(|| text_regex(&value)),
                        value,
                        written: text_regex(&written),
                    });
                }
                Term::Source(source) => matcher.sources.push(source),
            }
        }
        Ok(matcher)
    }

    fn source_matches(&mut self, source: u16, reader: &LogReader) -> bool {
        if self.sources.is_empty() {
            return true;
        }
        let index = source as usize;
        if index >= self.source_matches.len() {
            self.source_matches.resize(index + 1, None);
        }
        *self.source_matches[index].get_or_insert_with(|| {
            let Some(target) = reader.target(source) else {
                return false;
            };
            let label = format!("{}/{}", target.pod, target.container).to_lowercase();
            self.sources.iter().any(|source| label.contains(source))
        })
    }

    fn matches(&mut self, record: &Decoded, reader: &LogReader) -> bool {
        if record.gap {
            return true;
        }
        if record.severity < self.severity || !self.source_matches(record.source, reader) {
            return false;
        }
        if self.texts.is_empty() && self.fields.is_empty() {
            return true;
        }
        let message = split_timestamp(record.text).1;
        self.texts.iter().all(|text| text.is_match(message))
            && self.fields.iter().all(|field| field.matches(message))
    }
}

/// How far a scan has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterProgress {
    /// Matching lines found among those still stored.
    pub matches: usize,
    /// Lines checked so far, among those still stored.
    pub scanned: u64,
    /// Whether every stored line has been checked.
    pub complete: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub(super) struct Found {
    pub(super) matches: MatchSet,
    /// The matches counted over time, and the matching error lines.
    timeline: Timeline,
    /// The lines checked: older lines are added below, and new lines above.
    scanned: Range<u64>,
    held: Range<u64>,
}

impl Found {
    fn progress(&self) -> FilterProgress {
        FilterProgress {
            matches: self.matches.len(),
            scanned: self.scanned.end - self.scanned.start,
            complete: self.scanned == self.held,
            error: None,
        }
    }
}

/// Checks a store's lines a chunk at a time, new lines first and then older ones.
struct Scanner {
    reader: LogReader,
    matcher: Matcher,
    found: Arc<Mutex<Found>>,
    encoded: Vec<u8>,
    seqs: Vec<u64>,
    /// What each match adds to the timeline, with the segment that holds it.
    marks: Vec<(u64, u64, Mark)>,
}

impl Scanner {
    fn new(reader: LogReader, matcher: Matcher, held: Range<u64>) -> Self {
        Self {
            reader,
            matcher,
            found: Arc::new(Mutex::new(Found {
                matches: MatchSet::default(),
                scanned: held.end..held.end,
                held,
                ..Found::default()
            })),
            encoded: Vec::new(),
            seqs: Vec::new(),
            marks: Vec::new(),
        }
    }

    /// Checks one chunk of lines in `held`, the lines the store holds now. Returns whether
    /// lines are left to check.
    fn step(&mut self, held: Range<u64>) -> io::Result<bool> {
        let scanned = {
            let mut found = self.found.lock().expect("filter lock");
            found.matches.remove_below(held.start);
            found.timeline.remove_below(held.start);
            let start = found.scanned.start.max(held.start);
            found.scanned = start..found.scanned.end.max(start);
            found.held = held.clone();
            found.scanned.clone()
        };
        let range = if scanned.end < held.end {
            scanned.end..held.end.min(scanned.end + SCAN_LINES)
        } else if scanned.start > held.start {
            scanned.start.saturating_sub(SCAN_LINES).max(held.start)..scanned.start
        } else {
            return Ok(false);
        };
        self.check(range.clone())?;
        let mut found = self.found.lock().expect("filter lock");
        found.matches.extend(&self.seqs);
        for (part, seq, mark) in self.marks.drain(..) {
            found.timeline.add(part, seq, mark);
        }
        found.scanned = found.scanned.start.min(range.start)..found.scanned.end.max(range.end);
        Ok(found.scanned != held)
    }

    /// Finds the matches in `range` and leaves them in `seqs`.
    fn check(&mut self, mut range: Range<u64>) -> io::Result<()> {
        self.seqs.clear();
        self.marks.clear();
        while !range.is_empty() {
            let read = self
                .reader
                .store
                .lock()
                .expect("log store lock")
                .read_encoded(range.clone(), &mut self.encoded)?;
            let Some(read) = read else {
                return Ok(());
            };
            let (matcher, reader, seqs, marks) = (
                &mut self.matcher,
                &self.reader,
                &mut self.seqs,
                &mut self.marks,
            );
            each_record(
                read.first,
                &self.encoded,
                read.covered.clone(),
                |seq, record| {
                    if matcher.matches(&record, reader) {
                        seqs.push(seq);
                        if let Some(mark) = Mark::of(record.gap, record.severity, record.text) {
                            marks.push((read.segment, seq, mark));
                        }
                    }
                },
            )?;
            range.start = read.covered.end;
        }
        Ok(())
    }
}

/// A running filter over a tail's stored lines, which keeps up as lines arrive. Dropping
/// it stops the scan.
pub struct LogFilterScan {
    pub(super) reader: LogReader,
    pub(super) found: Arc<Mutex<Found>>,
    progress: watch::Receiver<FilterProgress>,
    _cancel: oneshot::Sender<()>,
}

/// Wakes when a scan finds more lines, or checks more.
pub struct FilterUpdates(watch::Receiver<FilterProgress>);

impl FilterUpdates {
    /// Waits for progress, or returns `None` once the scan has stopped.
    pub async fn next(&mut self) -> Option<FilterProgress> {
        self.0.changed().await.ok()?;
        Some(self.0.borrow_and_update().clone())
    }
}

impl LogReader {
    /// Starts finding the stored lines that `filter` matches, on a thread of its own.
    pub fn filter(&self, filter: &LogFilter) -> Result<LogFilterScan, FilterError> {
        let matcher = Matcher::new(filter)?;
        let mut held = self.store.lock().expect("log store lock").watch();
        let scanner = Scanner::new(self.clone(), matcher, held.borrow_and_update().clone());
        let found = scanner.found.clone();
        let (progress_sender, progress) =
            watch::channel(found.lock().expect("filter lock").progress());
        let (cancel, cancelled) = oneshot::channel();
        let started = thread::Builder::new()
            .name("taffrail-log-filter".to_owned())
            .spawn(move || scan(scanner, held, &progress_sender, cancelled));
        if let Err(error) = started {
            let mut progress = found.lock().expect("filter lock").progress();
            progress.error = Some(format!("Failed to start the filter: {error}"));
            let (_, receiver) = watch::channel(progress);
            return Ok(LogFilterScan {
                reader: self.clone(),
                found,
                progress: receiver,
                _cancel: cancel,
            });
        }
        Ok(LogFilterScan {
            reader: self.clone(),
            found,
            progress,
            _cancel: cancel,
        })
    }
}

fn scan(
    mut scanner: Scanner,
    mut held: watch::Receiver<Range<u64>>,
    progress: &watch::Sender<FilterProgress>,
    mut cancelled: oneshot::Receiver<()>,
) {
    loop {
        if !matches!(
            cancelled.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ) {
            return;
        }
        let range = held.borrow_and_update().clone();
        let more = match scanner.step(range) {
            Ok(more) => more,
            Err(error) => {
                let mut failed = scanner.found.lock().expect("filter lock").progress();
                failed.error = Some(format!("Failed to filter log lines: {error}"));
                progress.send_replace(failed);
                return;
            }
        };
        progress.send_replace(scanner.found.lock().expect("filter lock").progress());
        if !more {
            let changed = pin!(held.changed());
            match futures::executor::block_on(select(changed, &mut cancelled)) {
                Either::Left((Ok(()), _)) => {}
                _ => return,
            }
        }
    }
}

impl LogFilterScan {
    pub fn progress(&self) -> FilterProgress {
        self.progress.borrow().clone()
    }

    pub fn updates(&self) -> FilterUpdates {
        FilterUpdates(self.progress.clone())
    }

    /// Matching lines found so far.
    pub fn len(&self) -> usize {
        self.found.lock().expect("filter lock").matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many matching lines come before the line `seq`.
    pub fn position_of(&self, seq: u64) -> usize {
        self.found.lock().expect("filter lock").matches.rank(seq)
    }

    /// The sequence numbers of the matching lines at `positions`, oldest first.
    pub fn seqs(&self, positions: Range<usize>) -> Vec<u64> {
        self.found
            .lock()
            .expect("filter lock")
            .matches
            .select(positions)
    }

    /// The matching lines at `positions`, oldest first.
    pub fn lines(&self, positions: Range<usize>) -> io::Result<Vec<LogLine>> {
        let seqs = self.seqs(positions);
        self.reader
            .store
            .lock()
            .expect("log store lock")
            .read_seqs(&seqs)
    }

    /// Roughly how much memory the matches take, with their timeline.
    pub fn match_bytes(&self) -> usize {
        let found = self.found.lock().expect("filter lock");
        found.matches.bytes() + found.timeline.bytes()
    }

    /// The matching lines found so far, counted over time in at most `max_buckets` buckets.
    pub fn timeline(&self, max_buckets: usize) -> TimelineView {
        self.found
            .lock()
            .expect("filter lock")
            .timeline
            .view(max_buckets)
    }

    /// The first matching error line at or after `seq`.
    pub fn next_error(&self, seq: u64) -> Option<u64> {
        self.found
            .lock()
            .expect("filter lock")
            .timeline
            .next_error(seq)
    }

    /// The last matching error line before `seq`.
    pub fn previous_error(&self, seq: u64) -> Option<u64> {
        self.found
            .lock()
            .expect("filter lock")
            .timeline
            .previous_error(seq)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;
    use std::time::{Duration, Instant};

    use super::super::store::{Gap, LogStore, Record};
    use super::super::tail::LogTarget;
    use super::*;

    fn filter(query: &str) -> LogFilter {
        LogFilter {
            query: query.into(),
            severity: Severity::Unknown,
        }
    }

    fn reader(store: LogStore, pods: &[&str]) -> LogReader {
        let targets = pods
            .iter()
            .map(|pod| LogTarget {
                namespace: "shop".into(),
                pod: (*pod).into(),
                container: "app".into(),
            })
            .collect();
        LogReader {
            targets: Arc::new(RwLock::new(targets)),
            store: Arc::new(Mutex::new(store)),
        }
    }

    fn store_of(lines: &[(u16, &str)]) -> LogStore {
        let mut store = LogStore::new().unwrap();
        let records: Vec<Record> = lines
            .iter()
            .map(|(source, text)| Record::new(*source, text.as_bytes().to_vec()))
            .collect();
        store.append(&records).unwrap();
        store
    }

    fn decoded(line: &LogLine) -> Decoded<'_> {
        Decoded {
            source: line.source,
            severity: line.severity,
            gap: line.gap.is_some(),
            text: line.text.as_bytes(),
        }
    }

    /// Which of `lines` the filter keeps, by index.
    fn kept(query: &str, severity: Severity, lines: &[&str]) -> Vec<usize> {
        let reader = reader(
            store_of(&lines.iter().map(|line| (0, *line)).collect::<Vec<_>>()),
            &["api-1"],
        );
        let mut matcher = Matcher::new(&LogFilter {
            query: query.into(),
            severity,
        })
        .unwrap();
        reader
            .read(0..lines.len() as u64)
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, line)| matcher.matches(&decoded(line), &reader))
            .map(|(index, _)| index)
            .collect()
    }

    #[test]
    fn splits_a_query_into_terms() {
        assert_eq!(
            terms(
                r#" refused "no route"  /time(out|d)/ user_id=42 msg="a b" source:api /healthz "#
            ),
            [
                Term::Text("refused".into()),
                Term::Text("no route".into()),
                Term::Regex("time(out|d)".into()),
                Term::Field {
                    key: "user_id".into(),
                    value: "42".into(),
                    written: "user_id=42".into()
                },
                Term::Field {
                    key: "msg".into(),
                    value: "a b".into(),
                    written: r#"msg="a b""#.into()
                },
                Term::Source("api".into()),
                Term::Text("/healthz".into()),
            ]
        );
    }

    #[test]
    fn a_regular_expression_runs_to_a_slash_before_a_space() {
        assert_eq!(
            terms("/a b/ /v1/orders/ /x\\/y/"),
            [
                Term::Regex("a b".into()),
                Term::Regex("v1/orders".into()),
                Term::Regex("x/y".into()),
            ]
        );
        assert_eq!(terms("/v1/orders"), [Term::Text("/v1/orders".into())]);
    }

    #[test]
    fn empty_terms_are_ignored() {
        assert!(terms("  \"\" // ").is_empty());
        assert!(filter("   ").is_empty());
        assert!(!filter("x").is_empty());
        assert!(
            !LogFilter {
                query: String::new(),
                severity: Severity::Warning
            }
            .is_empty()
        );
    }

    const LINES: [&str; 6] = [
        "2026-11-07T10:42:06Z ERROR dial tcp: Connection refused",
        "2026-11-07T10:42:07Z WARN slow query took 1204ms",
        r#"2026-11-07T10:42:08Z {"level":"info","msg":"charged","user_id":42,"log":{"origin":"billing"}}"#,
        r#"2026-11-07T10:42:09Z {"level":"error","msg":"charge failed","user_id":"420"}"#,
        "2026-11-07T10:42:10Z time=10:42:10 level=info msg=done user_id=42",
        "2026-11-07T10:42:11Z connected after 3 attempts",
    ];

    #[test]
    fn text_matches_any_case_and_every_term_must_match() {
        assert_eq!(kept("connection REFUSED", Severity::Unknown, &LINES), [0]);
        assert_eq!(kept("\"query took\"", Severity::Unknown, &LINES), [1]);
        assert_eq!(
            kept("\"took query\"", Severity::Unknown, &LINES),
            [] as [usize; 0]
        );
        assert_eq!(kept("charge", Severity::Unknown, &LINES), [2, 3]);
    }

    #[test]
    fn text_skips_the_kubernetes_timestamp() {
        assert_eq!(
            kept("2026-11-07T10", Severity::Unknown, &LINES),
            [] as [usize; 0]
        );
        assert_eq!(kept("10:42:10", Severity::Unknown, &LINES), [4]);
    }

    #[test]
    fn a_regular_expression_ignores_case_unless_told_not_to() {
        assert_eq!(
            kept(r"/\d{4}ms|refused/", Severity::Unknown, &LINES),
            [0, 1]
        );
        assert_eq!(kept("/(?-i)Connection/", Severity::Unknown, &LINES), [0]);
        assert_eq!(kept("/(?-i)connect/", Severity::Unknown, &LINES), [5]);
    }

    #[test]
    fn an_invalid_regular_expression_is_an_error() {
        assert!(matches!(
            Matcher::new(&filter("/(unclosed/")),
            Err(FilterError::Regex(pattern)) if pattern == "(unclosed"
        ));
    }

    #[test]
    fn severity_keeps_lines_at_or_above_it() {
        assert_eq!(kept("", Severity::Warning, &LINES), [0, 1, 3]);
        assert_eq!(kept("", Severity::Error, &LINES), [0, 3]);
        assert_eq!(kept("", Severity::Debug, &LINES), [0, 1, 2, 3, 4]);
        assert_eq!(kept("charge", Severity::Error, &LINES), [3]);
    }

    #[test]
    fn a_field_matches_json_values_and_logfmt_text() {
        assert_eq!(kept("user_id=42", Severity::Unknown, &LINES), [2, 4]);
        assert_eq!(kept("user_id=420", Severity::Unknown, &LINES), [3]);
        assert_eq!(kept("level=ERROR", Severity::Unknown, &LINES), [3]);
        assert_eq!(kept("log.origin=billing", Severity::Unknown, &LINES), [2]);
        assert_eq!(
            kept(r#"msg="charge failed""#, Severity::Unknown, &LINES),
            [3]
        );
        assert_eq!(kept("msg=done", Severity::Unknown, &LINES), [4]);
    }

    #[test]
    fn a_json_line_without_the_field_does_not_match_on_its_text() {
        let lines = [r#"{"msg":"user_id=42 retried"}"#, "user_id=42 retried"];

        assert_eq!(kept("user_id=42", Severity::Unknown, &lines), [1]);
        assert_eq!(
            kept("user_id=42", Severity::Unknown, &[r#"{"broken": "#]),
            [] as [usize; 0]
        );
        assert_eq!(kept("broken=1", Severity::Unknown, &[r#"{"broken=1"#]), [0]);
    }

    #[test]
    fn source_terms_match_any_source_named() {
        let reader = reader(
            store_of(&[(0, "one"), (1, "two"), (2, "three"), (1, "four")]),
            &["api-1", "web-1", "db-0"],
        );
        let mut matcher = Matcher::new(&filter("source:WEB source:db")).unwrap();
        let kept: Vec<_> = reader
            .read(0..4)
            .unwrap()
            .into_iter()
            .filter(|line| matcher.matches(&decoded(line), &reader))
            .map(|line| line.text)
            .collect();

        assert_eq!(kept, ["two", "three", "four"]);
        let unknown = Decoded {
            source: 9,
            severity: Severity::Unknown,
            gap: false,
            text: b"x",
        };
        assert!(
            !Matcher::new(&filter("source:api"))
                .unwrap()
                .matches(&unknown, &reader)
        );
    }

    #[test]
    fn a_gap_shows_whatever_the_filter() {
        let gap = Gap {
            from: 1_794_048_126_000_000_000,
            to: 1_794_048_151_000_000_000,
        };
        let mut store = LogStore::new().unwrap();
        store
            .append(&[
                Record::new(0, b"ERROR payment failed".to_vec()),
                Record::gap(0, gap),
                Record::new(0, b"INFO ok".to_vec()),
            ])
            .unwrap();
        let reader = reader(store, &["api-1"]);
        let mut matcher = Matcher::new(&LogFilter {
            query: "nothing source:web".into(),
            severity: Severity::Error,
        })
        .unwrap();

        let kept: Vec<_> = reader
            .read(0..3)
            .unwrap()
            .iter()
            .filter(|line| matcher.matches(&decoded(line), &reader))
            .map(|line| line.seq)
            .collect();

        assert_eq!(kept, [1]);
    }

    fn numbered(count: usize) -> Vec<Record> {
        (0..count)
            .map(|index| {
                let text = if index % 10 == 0 {
                    format!("ERROR failed {index}")
                } else {
                    format!("INFO ok {index}")
                };
                Record::new((index % 3) as u16, text.into_bytes())
            })
            .collect()
    }

    fn scanner(store: LogStore, query: &str) -> (Scanner, LogReader) {
        let reader = reader(store, &["a", "b", "c"]);
        let held = reader.store.lock().unwrap().range();
        let scanner = Scanner::new(reader.clone(), Matcher::new(&filter(query)).unwrap(), held);
        (scanner, reader)
    }

    fn held(reader: &LogReader) -> Range<u64> {
        reader.store.lock().unwrap().range()
    }

    fn found(scanner: &Scanner) -> (Vec<u64>, FilterProgress) {
        let found = scanner.found.lock().unwrap();
        (found.matches.select(0..usize::MAX), found.progress())
    }

    #[test]
    fn a_scan_checks_the_newest_lines_first() {
        let lines = 3 * SCAN_LINES as usize;
        let mut store = LogStore::new().unwrap();
        store.append(&numbered(lines)).unwrap();
        let held = store.range();
        let (mut scanner, _) = scanner(store, "error");

        assert!(scanner.step(held.clone()).unwrap());
        let (seqs, progress) = found(&scanner);
        assert_eq!(progress.scanned, SCAN_LINES);
        assert!(!progress.complete);
        assert!(seqs.iter().all(|seq| *seq >= 2 * SCAN_LINES));
        assert_eq!(seqs.first(), Some(&(2 * SCAN_LINES).next_multiple_of(10)));

        while scanner.step(held.clone()).unwrap() {}
        let (seqs, progress) = found(&scanner);
        assert!(progress.complete);
        assert_eq!(progress.matches, lines.div_ceil(10));
        assert_eq!(seqs, (0..lines as u64).step_by(10).collect::<Vec<_>>());
    }

    #[test]
    fn new_lines_are_checked_before_older_ones() {
        let mut store = LogStore::new().unwrap();
        store.append(&numbered(2 * SCAN_LINES as usize)).unwrap();
        let (mut scanner, reader) = scanner(store, "error");

        scanner.step(held(&reader)).unwrap();
        reader
            .store
            .lock()
            .unwrap()
            .append(&[Record::new(0, b"ERROR new".to_vec())])
            .unwrap();
        let held = reader.store.lock().unwrap().range();
        scanner.step(held.clone()).unwrap();

        let (seqs, progress) = found(&scanner);
        assert_eq!(seqs.last(), Some(&(held.end - 1)));
        assert_eq!(progress.scanned, SCAN_LINES + 1);
    }

    #[test]
    fn a_scan_forgets_lines_the_store_drops() {
        let mut store = LogStore::with_limits(4096, 3).unwrap();
        store.append(&numbered(400)).unwrap();
        let (mut scanner, reader) = scanner(store, "error");
        while scanner.step(held(&reader)).unwrap() {}

        reader
            .store
            .lock()
            .unwrap()
            .append(&numbered(1_000))
            .unwrap();
        let held = reader.store.lock().unwrap().range();
        while scanner.step(held.clone()).unwrap() {}

        let (seqs, progress) = found(&scanner);
        assert!(progress.complete);
        assert!(seqs.iter().all(|seq| held.contains(seq)));
        let expected = reader
            .read(held)
            .unwrap()
            .into_iter()
            .filter(|line| line.text.starts_with("ERROR"))
            .count();
        assert_eq!(progress.matches, expected);
    }

    #[test]
    fn a_scan_counts_its_matches_over_time() {
        let mut store = LogStore::with_limits(4096, 3).unwrap();
        let stamped = |index: usize| {
            let level = if index.is_multiple_of(10) {
                "ERROR"
            } else {
                "INFO"
            };
            Record::new(
                0,
                format!("2026-11-07T10:42:{:02}Z {level} line {index}", index % 60).into_bytes(),
            )
        };
        store
            .append(&(0..400).map(stamped).collect::<Vec<_>>())
            .unwrap();
        let (mut scanner, reader) = scanner(store, "error");
        while scanner.step(held(&reader)).unwrap() {}

        let count = |scanner: &Scanner| {
            let found = scanner.found.lock().unwrap();
            (found.timeline.view(100).totals(), found.matches.len())
        };
        let (totals, matches) = count(&scanner);
        assert_eq!(totals.0, [matches as u64, 0, 0, 0]);
        let found = scanner.found.lock().unwrap();
        let first = found.matches.select(0..1)[0];
        assert_eq!(found.timeline.next_error(0), Some(first));
        drop(found);

        reader
            .store
            .lock()
            .unwrap()
            .append(&(400..1_400).map(stamped).collect::<Vec<_>>())
            .unwrap();
        let held = held(&reader);
        while scanner.step(held.clone()).unwrap() {}

        let (totals, matches) = count(&scanner);
        assert_eq!(
            totals.total(),
            matches as u64,
            "dropped matches leave the timeline"
        );
        let found = scanner.found.lock().unwrap();
        assert!(
            found
                .timeline
                .next_error(0)
                .is_some_and(|seq| seq >= held.start)
        );
    }

    async fn wait_for(
        scan: &LogFilterScan,
        done: impl Fn(&FilterProgress) -> bool,
    ) -> FilterProgress {
        let mut updates = scan.updates();
        let progress = scan.progress();
        if done(&progress) {
            return progress;
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let progress = updates.next().await.expect("the scan stopped");
                if done(&progress) {
                    return progress;
                }
            }
        })
        .await
        .expect("the scan never got there")
    }

    #[tokio::test]
    async fn a_running_scan_keeps_up_with_new_lines() {
        let mut store = LogStore::new().unwrap();
        store.append(&numbered(100)).unwrap();
        let reader = reader(store, &["a", "b", "c"]);
        let scan = reader.filter(&filter("error")).unwrap();
        wait_for(&scan, |progress| progress.complete).await;
        assert_eq!(scan.len(), 10);

        reader
            .store
            .lock()
            .unwrap()
            .append(&[
                Record::new(1, b"ERROR late".to_vec()),
                Record::new(1, b"INFO late".to_vec()),
            ])
            .unwrap();

        let progress = wait_for(&scan, |progress| {
            progress.matches == 11 && progress.complete
        })
        .await;
        assert_eq!(progress.scanned, 102);
        let lines = scan.lines(9..11).unwrap();
        assert_eq!(lines[0].text, "ERROR failed 90");
        assert_eq!(lines[1].text, "ERROR late");
        assert_eq!(scan.position_of(100), 10);
    }

    async fn wait_for_thread_to_stop(reader: &LogReader) {
        let started = Instant::now();
        while Arc::strong_count(&reader.store) > 1 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the scan never stopped"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn dropping_a_scan_stops_its_thread_while_it_waits_for_lines() {
        let reader = reader(store_of(&[(0, "ERROR one")]), &["a"]);
        let scan = reader.filter(&filter("error")).unwrap();
        wait_for(&scan, |progress| progress.complete).await;

        drop(scan);

        wait_for_thread_to_stop(&reader).await;
    }

    #[tokio::test]
    async fn dropping_a_scan_stops_it_part_way() {
        let mut store = LogStore::new().unwrap();
        store.append(&numbered(40 * SCAN_LINES as usize)).unwrap();
        let reader = reader(store, &["a", "b", "c"]);
        let scan = reader.filter(&filter("/f.i.e.d/")).unwrap();

        drop(scan);

        wait_for_thread_to_stop(&reader).await;
    }

    #[tokio::test]
    async fn the_first_matches_arrive_before_the_scan_ends() {
        let mut store = LogStore::new().unwrap();
        store.append(&numbered(20 * SCAN_LINES as usize)).unwrap();
        let reader = reader(store, &["a", "b", "c"]);
        let scan = reader.filter(&filter("error")).unwrap();

        let first = wait_for(&scan, |progress| progress.matches > 0).await;
        let last = wait_for(&scan, |progress| progress.complete).await;

        assert!(first.scanned < last.scanned);
        assert_eq!(last.matches, (20 * SCAN_LINES as usize).div_ceil(10));
    }

    /// Lines like a busy cluster's: request logs, JSON from several libraries, klog, and
    /// a few failures. `index` makes each line differ.
    fn realistic_line(index: u64) -> String {
        let stamp = format!(
            "2026-11-07T10:{:02}:{:02}.{:09}Z",
            index / 60_000 % 60,
            index / 1_000 % 60,
            index % 1_000 * 1_000_000
        );
        let user = index % 997;
        let message = match index % 16 {
            0 => format!(
                r#"{{"level":"info","ts":1794048126.118,"caller":"server/handler.go:88","msg":"request finished","method":"GET","path":"/v1/orders/{index}","status":200,"duration_ms":{},"user_id":{user}}}"#,
                index % 300
            ),
            1 => format!("INFO GET /healthz 200 {}ms request_id={index:x}", index % 9),
            2 => format!(
                r#"time="2026-11-07T10:42:06Z" level=info msg="cache refreshed" entries={} service=catalog"#,
                index % 5000
            ),
            3 => format!(
                "10.4.3.{} - - [07/Nov/2026:10:42:06 +0000] \"GET /v1/products/{index} HTTP/1.1\" 200 {} \"-\" \"checkout/2.31\"",
                index % 250,
                index % 9000
            ),
            4 => format!(
                r#"{{"level":"debug","msg":"cache hit","key":"product:{index}","user_id":{user}}}"#
            ),
            5 => format!(
                "I1107 10:42:06.118123       1 reflector.go:150] watch of *v1.Pod ended with {} items",
                index % 400
            ),
            6 => format!(
                r#"{{"severity":"INFO","message":"published event","topic":"orders","partition":{},"offset":{index}}}"#,
                index % 12
            ),
            7 => format!(
                "2026-11-07 10:42:06.118  INFO 1 --- [nio-8080-exec-{}] o.s.web.servlet.DispatcherServlet : Completed 200 OK",
                index % 20
            ),
            8 => format!(
                r#"{{"level":30,"time":1794048126118,"pid":1,"msg":"incoming request","req":{{"id":{index},"method":"POST","url":"/v1/charges"}},"user_id":{user}}}"#
            ),
            9 => format!(
                "DEBUG span exported trace_id={index:016x} spans={}",
                index % 40
            ),
            10 => format!(
                r#"{{"level":"warn","msg":"slow query","table":"orders_by_customer","ms":{},"user_id":{user}}}"#,
                1000 + index % 900
            ),
            11 => format!(
                "WARN retrying request to inventory-api attempt={} after {}ms",
                index % 5,
                index % 800
            ),
            12 => format!(
                "W1107 10:42:06.402911       1 controller.go:1126] Service \"storefront/catalog-{}\" does not have any active Endpoint",
                index % 30
            ),
            13 if index % 64 == 13 => format!(
                "ERROR dial tcp 10.4.12.8:5432: connect: connection refused (attempt {})",
                index % 7
            ),
            13 => format!(
                "INFO POST /v1/charges 201 {}ms request_id={index:x}",
                index % 200
            ),
            14 if index % 128 == 14 => format!(
                r#"{{"level":"error","msg":"charge failed","error":"card declined","status":502,"user_id":{user}}}"#
            ),
            14 => format!(
                r#"{{"level":"info","msg":"charge created","status":201,"amount":{},"user_id":{user}}}"#,
                index % 10_000
            ),
            _ => format!(
                "connected to postgres at 10.4.12.{}:5432 after {} attempts",
                index % 250,
                index % 4
            ),
        };
        format!("{stamp} {message}")
    }

    /// Run with `cargo test --release -p taffrail-core filter_speed -- --ignored --nocapture`.
    ///
    /// Fills a store with 1 GiB of lines from 50 sources, then times filters over it.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn filter_speed() {
        const SOURCES: u16 = 50;
        let budget = super::super::store::SEGMENT_BYTES * super::super::store::MAX_SEGMENTS as u64;
        let mut store = LogStore::new().unwrap();
        let started = Instant::now();
        let mut index = 0u64;
        let mut text_bytes = 0u64;
        while store.bytes() + 2 * 1024 * 1024 < budget {
            let batch: Vec<Record> = (0..1024)
                .map(|offset| {
                    let line = realistic_line(index + offset);
                    Record::new(
                        ((index + offset) % u64::from(SOURCES)) as u16,
                        line.into_bytes(),
                    )
                })
                .collect();
            text_bytes += batch
                .iter()
                .map(|record| record.text.len() as u64)
                .sum::<u64>();
            store.append(&batch).unwrap();
            index += 1024;
        }
        let held = store.range();
        assert_eq!(
            held.start, 0,
            "the store must not drop lines during the fill"
        );
        println!(
            "filled {} lines, {:.0} MiB stored ({:.0} MiB of text) from {SOURCES} sources in {:.1} s",
            held.end,
            store.bytes() as f64 / (1024. * 1024.),
            text_bytes as f64 / (1024. * 1024.),
            started.elapsed().as_secs_f64()
        );
        let pods: Vec<String> = (0..SOURCES)
            .map(|source| format!("checkout-api-{source}"))
            .collect();
        let pods: Vec<&str> = pods.iter().map(String::as_str).collect();
        let reader = reader(store, &pods);

        let cases = [
            ("text", filter("connection refused")),
            ("regex", filter(r"/status.:50\d/")),
            (
                "severity",
                LogFilter {
                    query: String::new(),
                    severity: Severity::Warning,
                },
            ),
            ("json field", filter("user_id=42")),
            ("source", filter("source:checkout-api-7")),
            (
                "text and error",
                LogFilter {
                    query: "charge".into(),
                    severity: Severity::Error,
                },
            ),
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (name, filter) in cases {
            for run in 0..2 {
                let started = Instant::now();
                let scan = reader.filter(&filter).unwrap();
                let (first, screen, all) = runtime.block_on(async {
                    let mut updates = scan.updates();
                    let (mut first, mut screen) = (None, None);
                    loop {
                        let progress = updates.next().await.unwrap();
                        if progress.matches > 0 {
                            first.get_or_insert(started.elapsed());
                        }
                        if progress.matches >= 50 {
                            screen.get_or_insert(started.elapsed());
                        }
                        if progress.complete {
                            return (first, screen, started.elapsed());
                        }
                    }
                });
                let ms = |elapsed: Option<Duration>| {
                    elapsed.map_or("-".to_owned(), |elapsed| {
                        format!("{:.1}", elapsed.as_secs_f64() * 1e3)
                    })
                };
                println!(
                    "{name:>14} run {run}: first match {} ms, first 50 {} ms, full scan {:.0} ms, {} matches in {:.1} KiB",
                    ms(first),
                    ms(screen),
                    all.as_secs_f64() * 1e3,
                    scan.len(),
                    scan.match_bytes() as f64 / 1024.
                );
            }
        }
    }
}
