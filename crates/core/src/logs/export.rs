//! Writes stored lines to a file the user chose, streaming from the store.
//!
//! Each line is written as `kubectl logs --prefix --timestamps` would print it:
//! `[pod/<pod>/<container>] ` and then the line as stored, its Kubernetes timestamp and
//! the bytes the container wrote, unchanged. Blocks of context are split by `--`, as
//! `grep` splits them. Memory stays bounded whatever the export's size: lines are read a
//! chunk at a time, and context keeps a window of `2 * REACH` lines.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use tokio::sync::watch;

use super::context::{Header, REACH, block_seqs};
use super::filter::{Found, LogFilterScan};
use super::store::{LogLine, each_record};
use super::tail::LogReader;

/// Lines or matches handled at a time.
const CHUNK: u64 = 8 * 1024;
const WRITE_BUFFER: usize = 1024 * 1024;
/// Splits blocks of context, as `grep` does.
pub const CONTEXT_SEPARATOR: &str = "--";

/// How far an export has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportProgress {
    /// Lines written so far.
    pub lines: u64,
    /// Lines, or matches with context, checked so far, out of `total`.
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

/// An export running on a thread of its own. Dropping it before it finishes stops it and
/// removes the partial file.
pub struct LogExport {
    progress: watch::Receiver<ExportProgress>,
    cancel: Arc<AtomicBool>,
}

impl LogExport {
    pub fn progress(&self) -> ExportProgress {
        self.progress.borrow().clone()
    }

    /// Waits for progress, or returns `None` once the export has stopped.
    pub async fn next(&mut self) -> Option<ExportProgress> {
        self.progress.changed().await.ok()?;
        Some(self.progress.borrow_and_update().clone())
    }
}

impl Drop for LogExport {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What to write: every held line, or a filter's matches with any context.
struct Export {
    reader: LogReader,
    found: Option<Arc<Mutex<Found>>>,
    context: usize,
    /// The lines held when the export started, so lines that arrive later are left out.
    held: Range<u64>,
    prefixes: Vec<Option<Vec<u8>>>,
}

/// Where the save dialog for an export opens: the Downloads folder, or home.
pub fn export_directory() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
}

/// The `[pod/<pod>/<container>] ` that starts each copied or exported line.
fn prefix(reader: &LogReader, source: u16) -> String {
    reader.target(source).map_or_else(String::new, |target| {
        format!("[pod/{}/{}] ", target.pod, target.container)
    })
}

impl LogReader {
    /// A line as copying and exporting write it: its source, then the line as stored.
    pub fn raw_line(&self, line: &LogLine) -> String {
        format!("{}{}", prefix(self, line.source), line.text)
    }

    /// Writes every stored line to `path`.
    pub fn export(&self, path: PathBuf) -> LogExport {
        start(
            Export {
                reader: self.clone(),
                found: None,
                context: 0,
                held: self.store.lock().expect("log store lock").range(),
                prefixes: Vec::new(),
            },
            path,
        )
    }
}

impl LogFilterScan {
    /// Writes the matching lines to `path`, each with up to `context` lines of its source
    /// around it.
    pub fn export(&self, context: usize, path: PathBuf) -> LogExport {
        start(
            Export {
                reader: self.reader.clone(),
                found: Some(self.found.clone()),
                context,
                held: self.reader.store.lock().expect("log store lock").range(),
                prefixes: Vec::new(),
            },
            path,
        )
    }
}

fn start(export: Export, path: PathBuf) -> LogExport {
    let (sender, progress) = watch::channel(ExportProgress::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let started = thread::Builder::new()
        .name("taffrail-log-export".to_owned())
        .spawn(move || {
            let written = create(&path).and_then(|file| {
                let mut out = BufWriter::with_capacity(WRITE_BUFFER, file);
                export.write(&mut out, &stop, &sender)?;
                out.flush()
            });
            let failed = written.err().map(|error| error.to_string());
            if failed.is_some() || stop.load(Ordering::Relaxed) {
                // A file cut short would look complete, so it goes.
                let _ = std::fs::remove_file(&path);
            }
            sender.send_modify(|progress| {
                progress.finished = true;
                progress.error = failed.map(|error| format!("Failed to export lines: {error}"));
            });
        });
    if let Err(error) = started {
        let (_, progress) = watch::channel(ExportProgress {
            finished: true,
            error: Some(format!("Failed to start the export: {error}")),
            ..ExportProgress::default()
        });
        return LogExport { progress, cancel };
    }
    LogExport { progress, cancel }
}

/// Creates or truncates `path`. A new file is readable by its owner alone on Unix, like the
/// store's own files, because log lines can hold anything a container prints.
fn create(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// The stored lines kept around the match being written, with their text.
#[derive(Default)]
struct Window {
    lines: VecDeque<(u64, u16, Vec<u8>)>,
    /// The next line to read into the window.
    next: u64,
}

impl Window {
    fn find(&self, seq: u64) -> Option<&(u64, u16, Vec<u8>)> {
        let at = self.lines.binary_search_by_key(&seq, |line| line.0).ok()?;
        self.lines.get(at)
    }

    fn headers(&self, range: Range<u64>) -> Vec<Header> {
        let start = self.lines.partition_point(|line| line.0 < range.start);
        self.lines
            .range(start..)
            .take_while(|line| line.0 < range.end)
            .map(|line| (line.0, line.1))
            .collect()
    }
}

impl Export {
    fn write(
        mut self,
        out: &mut impl Write,
        cancel: &AtomicBool,
        progress: &watch::Sender<ExportProgress>,
    ) -> io::Result<()> {
        match self.found.clone() {
            None => self.write_all(out, cancel, progress),
            Some(found) if self.context == 0 => self.write_matches(&found, out, cancel, progress),
            Some(found) => self.write_context(&found, out, cancel, progress),
        }
    }

    fn write_line(&mut self, source: u16, text: &[u8], out: &mut impl Write) -> io::Result<()> {
        let index = source as usize;
        if index >= self.prefixes.len() {
            self.prefixes.resize(index + 1, None);
        }
        let reader = &self.reader;
        let prefix =
            self.prefixes[index].get_or_insert_with(|| prefix(reader, source).into_bytes());
        out.write_all(prefix)?;
        out.write_all(text)?;
        out.write_all(b"\n")
    }

    fn write_all(
        &mut self,
        out: &mut impl Write,
        cancel: &AtomicBool,
        progress: &watch::Sender<ExportProgress>,
    ) -> io::Result<()> {
        let total = self.held.end - self.held.start;
        let mut next = self.held.start;
        let mut encoded = Vec::new();
        let mut lines = 0;
        while next < self.held.end && !cancel.load(Ordering::Relaxed) {
            let range = next..self.held.end.min(next + CHUNK);
            let read = self
                .reader
                .store
                .lock()
                .expect("log store lock")
                .read_encoded(range.clone(), &mut encoded)?;
            let Some(read) = read else {
                // Older lines were dropped while exporting: carry on from those held.
                let held = self.reader.store.lock().expect("log store lock").range();
                if held.start <= next {
                    break;
                }
                next = held.start;
                continue;
            };
            let mut failed = Ok(());
            each_record(read.first, &encoded, read.covered.clone(), |_, record| {
                if failed.is_ok() {
                    failed = self.write_line(record.source, record.text, out);
                    lines += 1;
                }
            })?;
            failed?;
            next = read.covered.end;
            progress.send_modify(|progress| {
                progress.lines = lines;
                progress.done = next - self.held.start;
                progress.total = total;
            });
        }
        Ok(())
    }

    /// The next `CHUNK` matches at or after `from`, below the end of the export.
    fn matches_from(&self, found: &Mutex<Found>, from: u64) -> Vec<u64> {
        let found = found.lock().expect("filter lock");
        let rank = found.matches.rank(from);
        let mut seqs = found.matches.select(rank..rank + CHUNK as usize);
        seqs.retain(|seq| *seq < self.held.end);
        seqs
    }

    fn total_matches(&self, found: &Mutex<Found>) -> u64 {
        let found = found.lock().expect("filter lock");
        (found.matches.rank(self.held.end) - found.matches.rank(self.held.start)) as u64
    }

    fn write_matches(
        &mut self,
        found: &Mutex<Found>,
        out: &mut impl Write,
        cancel: &AtomicBool,
        progress: &watch::Sender<ExportProgress>,
    ) -> io::Result<()> {
        let total = self.total_matches(found);
        let mut from = self.held.start;
        let mut lines = 0;
        while !cancel.load(Ordering::Relaxed) {
            let seqs = self.matches_from(found, from);
            let Some(last) = seqs.last().copied() else {
                break;
            };
            let mut read = Vec::with_capacity(seqs.len());
            self.reader
                .store
                .lock()
                .expect("log store lock")
                .each_seq(&seqs, |_, record| {
                    read.push((record.source, record.text.to_vec()));
                })?;
            for (source, text) in read {
                self.write_line(source, &text, out)?;
                lines += 1;
            }
            from = last + 1;
            progress.send_modify(|progress| {
                progress.lines = lines;
                progress.done += seqs.len() as u64;
                progress.total = total;
            });
        }
        Ok(())
    }

    fn write_context(
        &mut self,
        found: &Mutex<Found>,
        out: &mut impl Write,
        cancel: &AtomicBool,
        progress: &watch::Sender<ExportProgress>,
    ) -> io::Result<()> {
        let total = self.total_matches(found);
        let mut window = Window {
            lines: VecDeque::new(),
            next: self.held.start,
        };
        let mut previous: Option<Header> = None;
        let mut pending = self.matches_from(found, self.held.start);
        pending.reverse();
        let mut lines = 0;
        let mut done = 0;
        while let Some(seq) = pending.pop() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if pending.is_empty() {
                pending = self.matches_from(found, seq + 1);
                pending.reverse();
            }
            self.fill(&mut window, seq + 1 + REACH)?;
            while window
                .lines
                .front()
                .is_some_and(|line| line.0 + REACH < seq)
            {
                window.lines.pop_front();
            }
            let header = |seq: u64| window.find(seq).map(|line| (line.0, line.1));
            let Some(at) = header(seq) else {
                continue;
            };
            let next = match pending.last() {
                Some(next) if *next <= seq + REACH => header(*next),
                Some(next) => self.header_of(*next)?,
                None => None,
            };
            let held = self.reader.store.lock().expect("log store lock").range();
            let held = held.start..held.end.min(self.held.end);
            let block = block_seqs(previous, at, next, self.context, held, &mut |range| {
                Ok(window.headers(range))
            })?;
            if block.separated {
                out.write_all(CONTEXT_SEPARATOR.as_bytes())?;
                out.write_all(b"\n")?;
            }
            for wanted in block.before.iter().chain([&seq]).chain(&block.after) {
                if let Some((_, source, text)) = window.find(*wanted) {
                    self.write_line(*source, text, out)?;
                    lines += 1;
                }
            }
            previous = Some(at);
            done += 1;
            if done % CHUNK == 0 || pending.is_empty() {
                progress.send_modify(|progress| {
                    progress.lines = lines;
                    progress.done = done;
                    progress.total = total;
                });
            }
        }
        Ok(())
    }

    /// Reads lines into the window up to `end`, or the end of the export.
    fn fill(&self, window: &mut Window, end: u64) -> io::Result<()> {
        let end = end.min(self.held.end);
        let mut encoded = Vec::new();
        while window.next < end {
            let read = self
                .reader
                .store
                .lock()
                .expect("log store lock")
                .read_encoded(window.next..end.min(window.next + CHUNK), &mut encoded)?;
            let Some(read) = read else {
                let held = self.reader.store.lock().expect("log store lock").range();
                if held.start <= window.next {
                    return Ok(());
                }
                window.next = held.start;
                continue;
            };
            each_record(read.first, &encoded, read.covered.clone(), |seq, record| {
                window
                    .lines
                    .push_back((seq, record.source, record.text.to_vec()));
            })?;
            window.next = read.covered.end;
        }
        Ok(())
    }

    fn header_of(&self, seq: u64) -> io::Result<Option<Header>> {
        let mut header = None;
        self.reader
            .store
            .lock()
            .expect("log store lock")
            .each_seq(&[seq], |seq, record| header = Some((seq, record.source)))?;
        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;
    use std::time::Duration;

    use super::super::filter::LogFilter;
    use super::super::parse::Severity;
    use super::super::store::{LogStore, Record};
    use super::super::tail::LogTarget;
    use super::*;

    fn reader(lines: Vec<Record>) -> LogReader {
        let mut store = LogStore::new().unwrap();
        store.append(&lines).unwrap();
        let targets = ["api-1", "web-1"]
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

    async fn finish(mut export: LogExport) -> ExportProgress {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let progress = export.progress();
                if progress.finished {
                    return progress;
                }
                export.next().await.expect("the export stopped");
            }
        })
        .await
        .expect("the export never finished")
    }

    async fn scan(reader: &LogReader, query: &str) -> LogFilterScan {
        let scan = reader
            .filter(&LogFilter {
                query: query.into(),
                severity: Severity::Unknown,
            })
            .unwrap();
        let mut updates = scan.updates();
        while !scan.progress().complete {
            updates.next().await.unwrap();
        }
        scan
    }

    /// Lines 0 to 29, alternating between the two sources, with `ERROR` on 10 and 13.
    fn interleaved() -> Vec<Record> {
        (0..30)
            .map(|index| {
                let level = if index == 10 || index == 13 {
                    "ERROR"
                } else {
                    "INFO"
                };
                Record::new(
                    (index % 2) as u16,
                    format!("2026-11-07T10:42:{index:02}Z {level} line {index}").into_bytes(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn exports_every_line_as_stored_with_its_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("all.log");
        let reader = reader(vec![
            Record::new(0, b"2026-11-07T10:42:06Z ERROR boom".to_vec()),
            Record::new(1, vec![0xff, b' ', b'x']),
        ]);

        let progress = finish(reader.export(path.clone())).await;

        assert_eq!(progress.error, None);
        assert_eq!(progress.lines, 2);
        let mut expected =
            b"[pod/api-1/app] 2026-11-07T10:42:06Z ERROR boom\n[pod/web-1/app] ".to_vec();
        expected.extend_from_slice(&[0xff, b' ', b'x', b'\n']);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            expected,
            "bytes stay as written"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_export_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("all.log");
        let reader = reader(vec![Record::new(0, b"line".to_vec())]);

        finish(reader.export(path.clone())).await;

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn exports_only_the_matches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("errors.log");
        let reader = reader(interleaved());
        let scan = scan(&reader, "error").await;

        let progress = finish(scan.export(0, path.clone())).await;

        assert_eq!((progress.lines, progress.total), (2, 2));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[pod/api-1/app] 2026-11-07T10:42:10Z ERROR line 10\n\
             [pod/web-1/app] 2026-11-07T10:42:13Z ERROR line 13\n"
        );
    }

    #[tokio::test]
    async fn exports_context_as_the_view_shows_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("context.log");
        let reader = reader(interleaved());
        let scan = scan(&reader, "error").await;

        finish(scan.export(2, path.clone())).await;

        let written = std::fs::read_to_string(&path).unwrap();
        let mut shown = Vec::new();
        for position in 0..scan.len() {
            let block = scan.context(position, 2).unwrap().unwrap();
            if block.separated {
                shown.push("--".to_owned());
            }
            for line in block.lines {
                let target = reader.target(line.line.source).unwrap();
                shown.push(format!("[pod/{}/app] {}", target.pod, line.line.text));
            }
        }
        assert_eq!(written.lines().collect::<Vec<_>>(), shown);
        assert_eq!(
            written,
            "[pod/api-1/app] 2026-11-07T10:42:06Z INFO line 6\n\
             [pod/api-1/app] 2026-11-07T10:42:08Z INFO line 8\n\
             [pod/api-1/app] 2026-11-07T10:42:10Z ERROR line 10\n\
             [pod/api-1/app] 2026-11-07T10:42:12Z INFO line 12\n\
             [pod/api-1/app] 2026-11-07T10:42:14Z INFO line 14\n\
             --\n\
             [pod/web-1/app] 2026-11-07T10:42:09Z INFO line 9\n\
             [pod/web-1/app] 2026-11-07T10:42:11Z INFO line 11\n\
             [pod/web-1/app] 2026-11-07T10:42:13Z ERROR line 13\n\
             [pod/web-1/app] 2026-11-07T10:42:15Z INFO line 15\n\
             [pod/web-1/app] 2026-11-07T10:42:17Z INFO line 17\n"
        );
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_written_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing").join("x.log");
        let reader = reader(interleaved());

        let progress = finish(reader.export(path.clone())).await;

        assert!(
            progress
                .error
                .is_some_and(|error| error.starts_with("Failed to export lines: "))
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn dropping_an_export_removes_the_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cut.log");
        let lines: Vec<Record> = (0..200_000)
            .map(|index| Record::new(0, format!("INFO line {index}").into_bytes()))
            .collect();
        let reader = reader(lines);
        let export = reader.export(path.clone());
        let progress = export.progress.clone();

        drop(export);

        let mut progress = progress;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !progress.borrow().finished {
                progress.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(!path.exists());
    }

    /// Run with `/usr/bin/time -l cargo test --release -p taffrail-core export_speed -- --ignored --nocapture`
    /// to see the peak memory too.
    ///
    /// Fills a store with 1 GiB of lines, then exports all of them, and a filter's matches
    /// with 10 lines of context each.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn export_speed() {
        use std::time::Instant;

        const SOURCES: u16 = 50;
        let budget = super::super::store::SEGMENT_BYTES * super::super::store::MAX_SEGMENTS as u64;
        let mut store = LogStore::new().unwrap();
        let mut index = 0u64;
        while store.bytes() + 2 * 1024 * 1024 < budget {
            let batch: Vec<Record> = (0..1024)
                .map(|offset| {
                    let at = index + offset;
                    let level = if at % 64 == 13 { "ERROR" } else { "INFO" };
                    Record::new(
                        (at % u64::from(SOURCES)) as u16,
                        format!(
                            "2026-11-07T10:{:02}:{:02}.{:09}Z {level} request finished path=/v1/orders/{at} status=200 duration_ms={} user_id={}",
                            at / 60_000 % 60,
                            at / 1_000 % 60,
                            at % 1_000 * 1_000_000,
                            at % 300,
                            at % 997
                        )
                        .into_bytes(),
                    )
                })
                .collect();
            store.append(&batch).unwrap();
            index += 1024;
        }
        let stored = store.bytes();
        let targets = (0..SOURCES)
            .map(|source| LogTarget {
                namespace: "shop".into(),
                pod: format!("checkout-api-{source}"),
                container: "app".into(),
            })
            .collect();
        let reader = LogReader {
            targets: Arc::new(RwLock::new(targets)),
            store: Arc::new(Mutex::new(store)),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let scan = runtime.block_on(scan(&reader, "error"));
        type Start<'a> = Box<dyn Fn(PathBuf) -> LogExport + 'a>;
        let cases: [(&str, Start); 3] = [
            ("all lines", Box::new(|path| reader.export(path))),
            ("errors", Box::new(|path| scan.export(0, path))),
            (
                "errors with 10 lines around",
                Box::new(|path| scan.export(10, path)),
            ),
        ];
        for (name, export) in cases {
            let path = directory.path().join("export.log");
            let started = Instant::now();
            let progress = runtime.block_on(finish(export(path.clone())));
            let elapsed = started.elapsed().as_secs_f64();
            let written = std::fs::metadata(&path).unwrap().len();
            println!(
                "{name:>28}: {} lines, {:.0} MiB written in {elapsed:.2} s, from {:.0} MiB stored",
                progress.lines,
                written as f64 / (1024. * 1024.),
                stored as f64 / (1024. * 1024.),
            );
            assert_eq!(progress.error, None);
        }
    }
}
