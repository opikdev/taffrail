use std::io;
use std::ops::Range;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures::{AsyncBufRead, AsyncBufReadExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::LogParams;
use kube::{Api, Client};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::store::{LogLine, LogStore};

pub const MAX_TAIL_PODS: usize = 50;

const MAX_LINE_BYTES: usize = 16 * 1024;
const TRUNCATED_SUFFIX: &[u8] = b" [line truncated]";
const CHANNEL_LINES: usize = 1024;
const INITIAL_TAIL_LINES: i64 = 100;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const NOT_FOUND: u16 = 404;

type SourceLine = (u16, Vec<u8>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogTarget {
    pub namespace: String,
    pub pod: String,
    pub container: String,
}

/// How far a tail has got. `first..end` are the sequence numbers still held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogProgress {
    pub first: u64,
    pub end: u64,
    pub error: Option<String>,
}

/// A running log tail. Dropping it stops the streams and frees its disk space.
pub struct LogTail {
    reader: LogReader,
    progress: watch::Receiver<LogProgress>,
    _streams: JoinSet<()>,
}

impl LogTail {
    pub fn reader(&self) -> LogReader {
        self.reader.clone()
    }

    /// Waits for new lines, or returns `None` if nothing can arrive any more.
    pub async fn next(&mut self) -> Option<LogProgress> {
        self.progress.changed().await.ok()?;
        Some(self.progress.borrow_and_update().clone())
    }
}

/// Reads stored lines. Lines stay readable after the tail is dropped, until the reader is.
#[derive(Clone)]
pub struct LogReader {
    targets: Arc<[LogTarget]>,
    store: Arc<Mutex<LogStore>>,
}

impl LogReader {
    pub fn targets(&self) -> &[LogTarget] {
        &self.targets
    }

    pub fn read(&self, range: Range<u64>) -> io::Result<Vec<LogLine>> {
        self.store.lock().expect("log store lock").read(range)
    }
}

pub(crate) fn start(client: Client, targets: Vec<LogTarget>) -> io::Result<LogTail> {
    let store = Arc::new(Mutex::new(LogStore::new()?));
    let (lines, received) = mpsc::channel(CHANNEL_LINES);
    let (progress_sender, progress) = watch::channel(LogProgress::default());
    {
        let store = store.clone();
        thread::Builder::new()
            .name("taffrail-log-writer".to_owned())
            .spawn(move || write_lines(received, store, progress_sender))?;
    }
    let mut streams = JoinSet::new();
    for (source, target) in targets.iter().enumerate() {
        streams.spawn(follow_target(
            Api::namespaced(client.clone(), &target.namespace),
            source as u16,
            target.clone(),
            lines.clone(),
        ));
    }
    Ok(LogTail {
        reader: LogReader {
            targets: targets.into(),
            store,
        },
        progress,
        _streams: streams,
    })
}

fn write_lines(
    mut lines: mpsc::Receiver<SourceLine>,
    store: Arc<Mutex<LogStore>>,
    progress: watch::Sender<LogProgress>,
) {
    let mut batch = Vec::with_capacity(CHANNEL_LINES);
    while lines.blocking_recv_many(&mut batch, CHANNEL_LINES) > 0 {
        let mut store = store.lock().expect("log store lock");
        let appended = store.append(&batch);
        let held = store.range();
        drop(store);
        batch.clear();
        let error = appended
            .err()
            .map(|error| format!("Failed to store log lines: {error}"));
        let failed = error.is_some();
        progress.send_replace(LogProgress {
            first: held.start,
            end: held.end,
            error,
        });
        if failed {
            return;
        }
    }
}

async fn follow_target(
    pods: Api<Pod>,
    source: u16,
    target: LogTarget,
    lines: mpsc::Sender<SourceLine>,
) {
    let mut params = LogParams {
        container: Some(target.container.clone()),
        follow: true,
        timestamps: true,
        tail_lines: Some(INITIAL_TAIL_LINES),
        ..LogParams::default()
    };
    loop {
        let ended = follow_stream(&pods, &target.pod, &params, source, &lines).await;
        let not_found = matches!(&ended, Err(kube::Error::Api(status)) if status.code == NOT_FOUND);
        let reason = match ended {
            Ok(()) => "stream ended".to_owned(),
            Err(error) => error.to_string(),
        };
        let note = format!("[taffrail: {reason}]").into_bytes();
        if lines.send((source, note)).await.is_err() || not_found {
            return;
        }
        // Earlier lines are already stored, so a reconnect only wants new ones.
        params.tail_lines = Some(0);
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn follow_stream(
    pods: &Api<Pod>,
    pod: &str,
    params: &LogParams,
    source: u16,
    lines: &mpsc::Sender<SourceLine>,
) -> Result<(), kube::Error> {
    let stream = pods.log_stream(pod, params).await?;
    let mut stream = pin!(stream);
    let mut line = Vec::new();
    loop {
        match read_line(&mut stream, &mut line).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(error) => return Err(kube::Error::ReadEvents(error)),
        }
        if lines
            .send((source, std::mem::take(&mut line)))
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

/// Reads one line without its terminator, keeping at most `MAX_LINE_BYTES` of it.
///
/// Returns `false` at the end of the stream. A pod that never writes a newline
/// must not be able to grow this buffer without limit.
async fn read_line(
    stream: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> io::Result<bool> {
    let mut truncated = false;
    let mut read_any = false;
    loop {
        let available = stream.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        read_any = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let content = &available[..newline.unwrap_or(available.len())];
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&content[..room.min(content.len())]);
        truncated |= content.len() > room;
        let consumed = newline.map_or(available.len(), |newline| newline + 1);
        stream.consume_unpin(consumed);
        if newline.is_some() {
            break;
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if truncated {
        line.extend_from_slice(TRUNCATED_SUFFIX);
    }
    Ok(read_any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::io::Cursor;

    async fn read_all(input: &[u8]) -> Vec<Vec<u8>> {
        let mut stream = Cursor::new(input.to_vec());
        let mut lines = Vec::new();
        let mut line = Vec::new();
        while read_line(&mut stream, &mut line).await.unwrap() {
            lines.push(std::mem::take(&mut line));
        }
        lines
    }

    #[tokio::test]
    async fn splits_lines_and_strips_terminators() {
        let lines = read_all(b"one\ntwo\r\n\nlast").await;

        assert_eq!(lines, [b"one".as_slice(), b"two", b"", b"last"]);
    }

    #[tokio::test]
    async fn empty_input_has_no_lines() {
        assert!(read_all(b"").await.is_empty());
    }

    #[tokio::test]
    async fn keeps_a_line_at_the_limit_whole() {
        let exact = vec![b'a'; MAX_LINE_BYTES];
        let mut input = exact.clone();
        input.push(b'\n');

        assert_eq!(read_all(&input).await, [exact]);
    }

    #[tokio::test]
    async fn truncates_a_long_line_and_keeps_the_next_one() {
        let mut input = vec![b'a'; MAX_LINE_BYTES * 3];
        input.extend_from_slice(b"\nnext\n");

        let lines = read_all(&input).await;

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES + TRUNCATED_SUFFIX.len());
        assert!(lines[0].ends_with(TRUNCATED_SUFFIX));
        assert_eq!(lines[1], b"next");
    }

    fn writer() -> (
        mpsc::Sender<SourceLine>,
        Arc<Mutex<LogStore>>,
        watch::Receiver<LogProgress>,
        thread::JoinHandle<()>,
    ) {
        let store = Arc::new(Mutex::new(LogStore::new().unwrap()));
        let (lines, received) = mpsc::channel(4);
        let (progress_sender, progress) = watch::channel(LogProgress::default());
        let writer_store = store.clone();
        let writer = thread::spawn(move || write_lines(received, writer_store, progress_sender));
        (lines, store, progress, writer)
    }

    #[tokio::test]
    async fn the_writer_stores_every_line_past_a_small_channel() {
        let (lines, store, progress, writer) = writer();

        for index in 0..5_000u32 {
            lines
                .send(((index % 50) as u16, index.to_string().into_bytes()))
                .await
                .unwrap();
        }
        drop(lines);
        writer.join().unwrap();

        assert_eq!(progress.borrow().end, 5_000);
        assert_eq!(progress.borrow().error, None);
        let read = store.lock().unwrap().read(0..5_000).unwrap();
        assert_eq!(read.len(), 5_000);
        assert!(
            read.iter()
                .enumerate()
                .all(|(index, line)| line.text == index.to_string())
        );
    }

    #[tokio::test]
    async fn the_writer_stops_when_senders_are_gone() {
        let (lines, _store, progress, writer) = writer();

        drop(lines);
        writer.join().unwrap();

        assert_eq!(progress.borrow().end, 0);
    }
}
