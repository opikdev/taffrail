use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::ops::Range;
use std::pin::pin;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{AsyncBufRead, AsyncBufReadExt};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::jiff::Timestamp;
use kube::api::LogParams;
use kube::{Api, Client};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

use super::parse::split_timestamp;
use super::store::{Gap, LogLine, LogStore, Record};
use crate::follow::{Follow, Member};
use crate::pods::{PodFeed, PodKey, PodSnapshot};

pub const MAX_TAIL_PODS: usize = 50;

const MAX_LINE_BYTES: usize = 16 * 1024;
const TRUNCATED_SUFFIX: &[u8] = b" [line truncated]";
const CHANNEL_LINES: usize = 1024;
const INITIAL_TAIL_LINES: i64 = 100;
/// A pod that started after the tail is read from its first line. This only bounds a
/// pod whose clock or log makes it look newer than it is.
const NEW_POD_TAIL_LINES: i64 = 10_000;
/// A reconnect reads at most this many lines it missed. More than that is marked missing.
const RESUME_TAIL_LINES: i64 = NEW_POD_TAIL_LINES;
/// Bounds the lines remembered at the newest timestamp, for a container that writes
/// many lines with the same one.
const MAX_LINES_AT_LAST: usize = 4096;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
#[cfg(not(test))]
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
#[cfg(test)]
const RECONNECT_DELAY: Duration = Duration::from_millis(20);
const NOT_FOUND: u16 = 404;
/// Each line stores its source as a `u16`.
const MAX_SOURCES: usize = u16::MAX as usize + 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogTarget {
    pub namespace: String,
    pub pod: String,
    pub container: String,
}

/// What a follow has found in the newest pods.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Following {
    pub label: String,
    /// Pods being tailed, at most `MAX_TAIL_PODS`.
    pub pods: usize,
    /// Pods that match, which can be more than are tailed.
    pub matching: usize,
}

/// How far a tail has got. `first..end` are the sequence numbers still held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogProgress {
    pub first: u64,
    pub end: u64,
    /// Containers being tailed now.
    pub containers: usize,
    pub following: Option<Following>,
    pub error: Option<String>,
}

enum Driver {
    Fixed { _sources: Box<Sources> },
    Follow(JoinHandle<()>),
}

/// A running log tail. Dropping it stops the streams and frees its disk space.
pub struct LogTail {
    reader: LogReader,
    progress: watch::Receiver<LogProgress>,
    driver: Driver,
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

impl Drop for LogTail {
    fn drop(&mut self) {
        if let Driver::Follow(task) = &self.driver {
            task.abort();
        }
    }
}

/// Reads stored lines. Lines stay readable after the tail is dropped, until the reader is.
#[derive(Clone)]
pub struct LogReader {
    pub(super) targets: Arc<RwLock<Vec<LogTarget>>>,
    pub(super) store: Arc<Mutex<LogStore>>,
}

impl LogReader {
    /// The container that `source` names. A follow adds sources as pods join.
    pub fn target(&self, source: u16) -> Option<LogTarget> {
        self.targets
            .read()
            .expect("log targets lock")
            .get(source as usize)
            .cloned()
    }

    /// Every container this tail has followed, including any that have left.
    pub fn source_count(&self) -> usize {
        self.targets.read().expect("log targets lock").len()
    }

    pub fn read(&self, range: Range<u64>) -> io::Result<Vec<LogLine>> {
        self.store.lock().expect("log store lock").read(range)
    }
}

struct Running {
    uid: String,
    stream: AbortHandle,
}

/// The streams of a tail, which a follow changes as pods come and go.
///
/// A container keeps its source number for the life of the tail, so its stored lines
/// keep their name after it leaves.
struct Sources {
    client: Client,
    lines: mpsc::Sender<Record>,
    targets: Arc<RwLock<Vec<LogTarget>>>,
    ids: HashMap<LogTarget, u16>,
    running: HashMap<u16, Running>,
    streams: JoinSet<()>,
}

impl Sources {
    fn new(client: Client, lines: mpsc::Sender<Record>) -> Self {
        Self {
            client,
            lines,
            targets: Arc::default(),
            ids: HashMap::new(),
            running: HashMap::new(),
            streams: JoinSet::new(),
        }
    }

    /// Streams exactly `members`, starting new ones and stopping those left out.
    /// A pod that comes back under the same name with a new uid gets a new stream.
    /// Returns `false` if the tail has no source numbers left for a new container.
    fn set(&mut self, members: Vec<Member>) -> bool {
        let mut wanted = HashSet::new();
        let mut room = true;
        for member in members {
            let Some(source) = self.source(&member.target) else {
                room = false;
                continue;
            };
            wanted.insert(source);
            if self
                .running
                .get(&source)
                .is_some_and(|running| running.uid == member.uid)
            {
                continue;
            }
            if let Some(replaced) = self.running.remove(&source) {
                replaced.stream.abort();
            }
            let stream = self.streams.spawn(follow_target(
                Api::namespaced(self.client.clone(), &member.target.namespace),
                source,
                member.target,
                member.from_start,
                self.lines.clone(),
            ));
            self.running.insert(
                source,
                Running {
                    uid: member.uid,
                    stream,
                },
            );
        }
        self.running.retain(|source, running| {
            let keep = wanted.contains(source);
            if !keep {
                running.stream.abort();
            }
            keep
        });
        while self.streams.try_join_next().is_some() {}
        room
    }

    fn source(&mut self, target: &LogTarget) -> Option<u16> {
        if let Some(source) = self.ids.get(target) {
            return Some(*source);
        }
        let mut targets = self.targets.write().expect("log targets lock");
        if targets.len() >= MAX_SOURCES {
            return None;
        }
        let source = targets.len() as u16;
        targets.push(target.clone());
        self.ids.insert(target.clone(), source);
        Some(source)
    }

    fn pods(&self) -> HashSet<PodKey> {
        let targets = self.targets.read().expect("log targets lock");
        self.running
            .keys()
            .map(|source| {
                let target = &targets[*source as usize];
                (target.namespace.clone(), target.pod.clone())
            })
            .collect()
    }
}

/// A new store, with its writer thread running.
struct OpenStore {
    store: Arc<Mutex<LogStore>>,
    /// Where to send lines for the writer to store.
    lines: mpsc::Sender<Record>,
    progress_sender: Arc<watch::Sender<LogProgress>>,
    progress: watch::Receiver<LogProgress>,
}

fn open_store() -> io::Result<OpenStore> {
    let store = Arc::new(Mutex::new(LogStore::new()?));
    let (lines, received) = mpsc::channel(CHANNEL_LINES);
    let (progress_sender, progress) = watch::channel(LogProgress::default());
    let progress_sender = Arc::new(progress_sender);
    {
        let store = store.clone();
        let progress_sender = progress_sender.clone();
        thread::Builder::new()
            .name("taffrail-log-writer".to_owned())
            .spawn(move || write_lines(received, store, &progress_sender))?;
    }
    Ok(OpenStore {
        store,
        lines,
        progress_sender,
        progress,
    })
}

/// Tails a fixed list of containers. Must run inside the Tokio runtime.
pub(crate) fn start(client: Client, targets: Vec<LogTarget>) -> io::Result<LogTail> {
    let OpenStore {
        store,
        lines,
        progress_sender,
        progress,
    } = open_store()?;
    let mut sources = Sources::new(client, lines);
    let containers = targets.len();
    sources.set(
        targets
            .into_iter()
            .map(|target| Member {
                target,
                uid: String::new(),
                from_start: false,
            })
            .collect(),
    );
    progress_sender.send_modify(|progress| progress.containers = containers);
    Ok(LogTail {
        reader: LogReader {
            targets: sources.targets.clone(),
            store,
        },
        progress,
        driver: Driver::Fixed {
            _sources: Box::new(sources),
        },
    })
}

/// Tails the pods that `follow` matches in `feed`, as they change. Must run inside the
/// Tokio runtime.
pub(crate) fn follow(client: Client, feed: PodFeed, follow: Follow) -> io::Result<LogTail> {
    let OpenStore {
        store,
        lines,
        progress_sender,
        progress,
    } = open_store()?;
    let sources = Sources::new(client, lines);
    let reader = LogReader {
        targets: sources.targets.clone(),
        store,
    };
    let task = tokio::spawn(follow_pods(sources, follow, feed.0, progress_sender));
    Ok(LogTail {
        reader,
        progress,
        driver: Driver::Follow(task),
    })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

async fn follow_pods(
    mut sources: Sources,
    follow: Follow,
    mut feed: watch::Receiver<Arc<PodSnapshot>>,
    progress: Arc<watch::Sender<LogProgress>>,
) {
    let started = unix_now();
    loop {
        let snapshot = feed.borrow_and_update().clone();
        if snapshot.synced {
            let chosen = follow.choose(&snapshot, &sources.pods(), started);
            let containers = chosen.members.len();
            let room = sources.set(chosen.members);
            progress.send_modify(|progress| {
                progress.containers = containers;
                progress.following = Some(Following {
                    label: follow.label.clone(),
                    pods: chosen.pods,
                    matching: chosen.matching,
                });
                if !room {
                    progress.error = Some(format!(
                        "This tail has followed {MAX_SOURCES} containers, the most one tail can. Tail again to follow new pods."
                    ));
                }
            });
        }
        if feed.changed().await.is_err() {
            // The session has ended. Keep the streams until the tail is dropped.
            std::future::pending::<()>().await;
        }
    }
}

fn write_lines(
    mut lines: mpsc::Receiver<Record>,
    store: Arc<Mutex<LogStore>>,
    progress: &watch::Sender<LogProgress>,
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
        progress.send_modify(|progress| {
            progress.first = held.start;
            progress.end = held.end;
            if error.is_some() {
                progress.error = error;
            }
        });
        if failed {
            return;
        }
    }
}

/// Where a source's stored lines end, so a reconnect can resume after them.
///
/// A reconnect asks for lines since the whole second of the last one, so it can send
/// stored lines again. Those are skipped until the stream passes the last timestamp.
#[derive(Debug, Default)]
struct Resume {
    /// The newest Kubernetes timestamp stored.
    last: Option<i64>,
    /// Hashes of the lines stored at `last`, with how many of each.
    at_last: HashMap<u64, u32>,
    /// The lines at `last` this stream may send again, until it passes `last`.
    replay: Option<HashMap<u64, u32>>,
    /// Set from a reconnect until a line at or before `last` shows that the stream
    /// overlaps the stored lines. A newer line first means lines may be missing.
    checking: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Skip,
    Store(Option<Gap>),
}

impl Resume {
    /// A new stream starts, which may repeat stored lines.
    fn start_stream(&mut self) {
        self.replay = self.last.map(|_| self.at_last.clone());
    }

    /// The next stream should send the last stored line again, or lines may be missing.
    fn expect_overlap(&mut self) {
        self.checking = self.last.is_some();
    }

    /// The second to ask for lines since, which includes the last stored line.
    fn since(&self) -> Option<Timestamp> {
        let last = self.last?;
        Timestamp::from_second(last.div_euclid(NANOS_PER_SECOND)).ok()
    }

    /// Marks the lines after the last stored one as missing up to `to`.
    fn close_gap(&mut self, to: i64) -> Option<Gap> {
        let from = self.last.filter(|from| self.checking && *from < to)?;
        self.checking = false;
        Some(Gap { from, to })
    }

    /// Whether to store `line`, and any gap the stream shows before it.
    fn admit(&mut self, line: &[u8]) -> Admit {
        let Some(stamp) = split_timestamp(line).0 else {
            return Admit::Store(None);
        };
        let hash = line_hash(line);
        let mut gap = None;
        match self.last {
            Some(last) if stamp <= last => {
                self.checking = false;
                if let Some(replay) = &mut self.replay {
                    if stamp < last {
                        return Admit::Skip;
                    }
                    if let Some(count) = replay.get_mut(&hash).filter(|count| **count > 0) {
                        *count -= 1;
                        return Admit::Skip;
                    }
                }
                if stamp == last {
                    remember(&mut self.at_last, hash);
                }
                return Admit::Store(None);
            }
            Some(_) => {
                gap = self.close_gap(stamp);
                self.replay = None;
            }
            None => {}
        }
        self.last = Some(stamp);
        self.at_last.clear();
        self.at_last.insert(hash, 1);
        Admit::Store(gap)
    }
}

fn line_hash(line: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

fn remember(lines: &mut HashMap<u64, u32>, hash: u64) {
    if let Some(count) = lines.get_mut(&hash) {
        *count += 1;
    } else if lines.len() < MAX_LINES_AT_LAST {
        lines.insert(hash, 1);
    }
}

fn unix_now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as i64)
}

/// One run of a container, so a reconnect can tell that it restarted.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Instance {
    pod_uid: String,
    container_id: String,
    restarts: i32,
    /// When this run started, in nanoseconds since the Unix epoch.
    started: Option<i64>,
}

impl Instance {
    fn replaced_by(&self, other: &Self) -> bool {
        self.pod_uid != other.pod_uid
            || self.restarts != other.restarts
            || (!self.container_id.is_empty()
                && !other.container_id.is_empty()
                && self.container_id != other.container_id)
    }
}

/// The run of `target` that its pod shows now, if it shows one.
async fn container_instance(
    pods: &Api<Pod>,
    target: &LogTarget,
) -> Result<Option<Instance>, kube::Error> {
    let pod = pods.get(&target.pod).await?;
    let pod_uid = pod.metadata.uid.unwrap_or_default();
    let Some(status) = pod.status.and_then(|status| {
        status
            .container_statuses
            .into_iter()
            .flatten()
            .chain(status.init_container_statuses.into_iter().flatten())
            .find(|container| container.name == target.container)
    }) else {
        return Ok(None);
    };
    let started = status
        .state
        .and_then(|state| state.running?.started_at)
        .map(|started| started.0.as_nanosecond() as i64);
    Ok(Some(Instance {
        pod_uid,
        container_id: status.container_id.unwrap_or_default(),
        restarts: status.restart_count,
        started,
    }))
}

fn is_not_found(error: &kube::Error) -> bool {
    matches!(error, kube::Error::Api(status) if status.code == NOT_FOUND)
}

/// Why a stream stopped.
enum End {
    Finished,
    Failed(kube::Error),
    /// The store is gone, so nothing more is wanted.
    Closed,
}

/// Follows one container: reconnects when a stream ends, resumes after the last stored
/// line, and reads the previous run of a container that restarted.
struct Follower {
    pods: Api<Pod>,
    source: u16,
    target: LogTarget,
    lines: mpsc::Sender<Record>,
    resume: Resume,
    /// Lines stored, other than Taffrail's own.
    stored: u64,
}

impl Follower {
    async fn send(&self, record: Record) -> bool {
        self.lines.send(record).await.is_ok()
    }

    async fn send_gap(&self, gap: Option<Gap>) -> bool {
        match gap {
            Some(gap) => self.send(Record::gap(self.source, gap)).await,
            None => true,
        }
    }

    fn params(&self) -> LogParams {
        LogParams {
            container: Some(self.target.container.clone()),
            timestamps: true,
            ..LogParams::default()
        }
    }

    /// Reads one stream to its end. `opened` is set once the API accepts the request.
    async fn stream(&mut self, params: &LogParams, opened: &mut bool) -> End {
        let stream = match self.pods.log_stream(&self.target.pod, params).await {
            Ok(stream) => stream,
            Err(error) => return End::Failed(error),
        };
        *opened = true;
        self.resume.start_stream();
        let mut stream = pin!(stream);
        let mut line = Vec::new();
        loop {
            match read_line(&mut stream, &mut line).await {
                Ok(true) => {}
                Ok(false) => return End::Finished,
                Err(error) => return End::Failed(kube::Error::ReadEvents(error)),
            }
            let text = std::mem::take(&mut line);
            let Admit::Store(gap) = self.resume.admit(&text) else {
                continue;
            };
            // Each stream finds its own lines' severity, so the work spreads across the
            // runtime's threads and stays out of the store's lock.
            if !self.send_gap(gap).await || !self.send(Record::new(self.source, text)).await {
                return End::Closed;
            }
            self.stored += 1;
        }
    }

    /// Reads what the previous run wrote after the last stored line. A cluster that
    /// keeps no previous log answers with an error, which leaves the gap to be marked.
    async fn read_previous(&mut self) -> bool {
        let params = LogParams {
            previous: true,
            since_time: self.resume.since(),
            tail_lines: Some(RESUME_TAIL_LINES),
            ..self.params()
        };
        !matches!(self.stream(&params, &mut false).await, End::Closed)
    }

    async fn run(mut self, from_start: bool) {
        let mut params = LogParams {
            follow: true,
            tail_lines: Some(if from_start {
                NEW_POD_TAIL_LINES
            } else {
                INITIAL_TAIL_LINES
            }),
            ..self.params()
        };
        let mut instance: Option<Instance> = None;
        let mut opened = false;
        let mut failed = false;
        let mut last_note = None;
        loop {
            let current = match container_instance(&self.pods, &self.target).await {
                Ok(current) => current,
                Err(error) if is_not_found(&error) => {
                    self.gone(failed, &error).await;
                    return;
                }
                // A role may read logs without reading pods. Restarts then go unseen.
                Err(_) => None,
            };
            if opened {
                self.resume.expect_overlap();
                let replaced = instance
                    .as_ref()
                    .zip(current.as_ref())
                    .is_some_and(|(stored, current)| stored.replaced_by(current));
                params.since_time = self.resume.since();
                params.tail_lines = Some(RESUME_TAIL_LINES);
                if replaced {
                    let same_pod = instance.as_ref().map(|stored| &stored.pod_uid)
                        == current.as_ref().map(|current| &current.pod_uid);
                    if same_pod && !self.read_previous().await {
                        return;
                    }
                    let started = current.as_ref().and_then(|current| current.started);
                    let gap = started.and_then(|started| self.resume.close_gap(started));
                    if !self.send_gap(gap).await {
                        return;
                    }
                    // The new run starts after every stored line, so read it from its start.
                    params.since_time = None;
                }
            }
            if current.is_some() {
                instance = current;
            }
            let stored = self.stored;
            let ended = self.stream(&params, &mut opened).await;
            let reason = match ended {
                End::Closed => return,
                End::Finished => {
                    failed = false;
                    "tail ended".to_owned()
                }
                End::Failed(error) if is_not_found(&error) => {
                    self.gone(failed, &error).await;
                    return;
                }
                End::Failed(error) => {
                    failed = true;
                    error.to_string()
                }
            };
            if self.stored > stored {
                last_note = None;
            }
            // A pod that is still starting fails the same way until it runs, and a
            // finished container ends the same way on each reconnect, so say it once.
            let note = format!("[taffrail: {reason}]");
            if last_note.as_ref() != Some(&note) {
                if !self
                    .send(Record::note(self.source, note.clone().into_bytes()))
                    .await
                {
                    return;
                }
                last_note = Some(note);
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }

    /// The pod is gone. Lines it wrote after a failed stream may be missing.
    async fn gone(&mut self, failed: bool, error: &kube::Error) {
        if failed {
            self.resume.expect_overlap();
            let gap = self.resume.close_gap(unix_now_nanos());
            if !self.send_gap(gap).await {
                return;
            }
        }
        let note = format!("[taffrail: {error}]");
        self.send(Record::note(self.source, note.into_bytes()))
            .await;
    }
}

/// Follows one container until it is gone. `from_start` reads its log from the first
/// line, for a pod that started after the tail did.
async fn follow_target(
    pods: Api<Pod>,
    source: u16,
    target: LogTarget,
    from_start: bool,
    lines: mpsc::Sender<Record>,
) {
    Follower {
        pods,
        source,
        target,
        lines,
        resume: Resume::default(),
        stored: 0,
    }
    .run(from_start)
    .await;
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
        mpsc::Sender<Record>,
        Arc<Mutex<LogStore>>,
        watch::Receiver<LogProgress>,
        thread::JoinHandle<()>,
    ) {
        let store = Arc::new(Mutex::new(LogStore::new().unwrap()));
        let (lines, received) = mpsc::channel(4);
        let (progress_sender, progress) = watch::channel(LogProgress::default());
        let writer_store = store.clone();
        let writer = thread::spawn(move || write_lines(received, writer_store, &progress_sender));
        (lines, store, progress, writer)
    }

    #[tokio::test]
    async fn the_writer_stores_every_line_past_a_small_channel() {
        let (lines, store, progress, writer) = writer();

        for index in 0..5_000u32 {
            lines
                .send(Record::new(
                    (index % 50) as u16,
                    index.to_string().into_bytes(),
                ))
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

    mod following {
        use std::collections::BTreeMap;

        use super::*;
        use crate::fake_api::{self, log_line};
        use crate::pods::PodRow;
        use crate::selector::Selector;

        fn pod(name: &str, app: &str) -> Arc<PodRow> {
            Arc::new(PodRow {
                namespace: "shop".into(),
                name: name.into(),
                uid: format!("uid-{name}"),
                created_unix: Some(1),
                containers: vec!["app".into()],
                labels: BTreeMap::from([("app".into(), app.into())]),
                ..PodRow::default()
            })
        }

        fn snapshot(rows: Vec<Arc<PodRow>>) -> Arc<PodSnapshot> {
            let mut rows = rows;
            rows.sort_by_key(|row| row.key());
            Arc::new(PodSnapshot {
                rows,
                synced: true,
                error: None,
            })
        }

        async fn wait_for(
            tail: &mut LogTail,
            mut done: impl FnMut(&LogProgress, &LogReader) -> bool,
        ) -> LogProgress {
            let reader = tail.reader();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let progress = tail.next().await.expect("the tail ended");
                    if done(&progress, &reader) {
                        return progress;
                    }
                }
            })
            .await
            .expect("the tail never got there")
        }

        fn texts(reader: &LogReader, progress: &LogProgress) -> Vec<String> {
            reader
                .read(progress.first..progress.end)
                .unwrap()
                .into_iter()
                .map(|line| line.text)
                .collect()
        }

        fn has_line(reader: &LogReader, progress: &LogProgress, pod: &str) -> bool {
            texts(reader, progress).contains(&log_line(pod))
        }

        #[tokio::test]
        async fn pods_that_appear_join_and_pods_that_go_leave() {
            let connection = fake_api::connect(Vec::new()).await;
            let (feed, received) =
                watch::channel(snapshot(vec![pod("api-1", "api"), pod("web-1", "web")]));
            let selector =
                Follow::selector(Some("shop".into()), Selector::parse("app=api").unwrap());
            let mut tail = follow(connection.client, PodFeed(received), selector).unwrap();

            let progress = wait_for(&mut tail, |progress, reader| {
                has_line(reader, progress, "api-1")
            })
            .await;
            assert_eq!(
                progress.following,
                Some(Following {
                    label: "app=api in shop".into(),
                    pods: 1,
                    matching: 1,
                })
            );
            assert_eq!(progress.containers, 1);
            assert!(!has_line(&tail.reader(), &progress, "web-1"));

            feed.send(snapshot(vec![pod("api-1", "api"), pod("api-2", "api")]))
                .unwrap();
            let progress = wait_for(&mut tail, |progress, reader| {
                has_line(reader, progress, "api-2")
            })
            .await;
            assert_eq!(progress.following.as_ref().unwrap().pods, 2);
            assert_eq!(progress.containers, 2);

            feed.send(snapshot(vec![pod("api-2", "api")])).unwrap();
            let progress = wait_for(&mut tail, |progress, _| progress.containers == 1).await;
            assert_eq!(progress.following.as_ref().unwrap().pods, 1);

            let reader = tail.reader();
            let names: Vec<_> = (0..reader.source_count() as u16)
                .map(|source| reader.target(source).unwrap().pod)
                .collect();
            assert_eq!(names, ["api-1", "api-2"], "a pod that left keeps its name");
        }

        #[tokio::test]
        async fn a_follow_waits_for_the_first_full_list() {
            let connection = fake_api::connect(Vec::new()).await;
            let (feed, received) = watch::channel(Arc::new(PodSnapshot::default()));
            let selector = Follow::selector(None, Selector::parse("app=api").unwrap());
            let mut tail = follow(connection.client, PodFeed(received), selector).unwrap();

            feed.send(snapshot(vec![pod("api-1", "api")])).unwrap();
            let progress = wait_for(&mut tail, |progress, _| progress.following.is_some()).await;

            assert_eq!(progress.following.unwrap().pods, 1);
        }

        #[tokio::test]
        async fn a_pod_recreated_under_its_name_gets_a_new_stream() {
            let connection = fake_api::connect(Vec::new()).await;
            let (lines, _received) = mpsc::channel(CHANNEL_LINES);
            let mut sources = Sources::new(connection.client, lines);
            let member = |uid: &str| Member {
                target: LogTarget {
                    namespace: "shop".into(),
                    pod: "db-0".into(),
                    container: "app".into(),
                },
                uid: uid.into(),
                from_start: true,
            };

            sources.set(vec![member("first")]);
            let first = sources.running[&0].stream.id();
            sources.set(vec![member("first")]);
            let same = sources.running[&0].stream.id();
            sources.set(vec![member("second")]);
            let second = sources.running[&0].stream.id();

            assert_eq!(first, same);
            assert_ne!(first, second);
            assert_eq!(sources.targets.read().unwrap().len(), 1);
        }
    }

    #[test]
    fn a_stream_that_passes_the_last_line_stops_skipping() {
        let mut resume = Resume::default();
        assert_eq!(resume.admit(b"2026-01-01T00:00:05Z a"), Admit::Store(None));
        resume.start_stream();

        assert_eq!(resume.admit(b"2026-01-01T00:00:04Z old"), Admit::Skip);
        assert_eq!(resume.admit(b"2026-01-01T00:00:05Z a"), Admit::Skip);
        assert_eq!(resume.admit(b"2026-01-01T00:00:06Z b"), Admit::Store(None));
        assert_eq!(
            resume.admit(b"2026-01-01T00:00:04Z clock stepped back"),
            Admit::Store(None),
            "once past the stored lines, every line is new"
        );
        assert_eq!(resume.admit(b"no timestamp"), Admit::Store(None));
    }

    #[test]
    fn a_reconnect_that_skips_the_last_line_shows_a_gap() {
        let mut resume = Resume::default();
        resume.admit(b"2026-01-01T00:00:05.5Z a");
        resume.expect_overlap();
        resume.start_stream();

        assert_eq!(
            resume.since(),
            Some(Timestamp::from_second(1_767_225_605).unwrap())
        );
        assert_eq!(
            resume.admit(b"2026-01-01T00:00:09Z b"),
            Admit::Store(Some(Gap {
                from: 1_767_225_605_500_000_000,
                to: 1_767_225_609_000_000_000,
            }))
        );
        assert_eq!(resume.admit(b"2026-01-01T00:00:10Z c"), Admit::Store(None));
    }

    mod resuming {
        use std::collections::HashMap;

        use super::super::super::parse::parse_rfc3339;
        use super::*;
        use crate::fake_api::{self, Reply, Request};

        const POD_PATH: &str = "/api/v1/namespaces/shop/pods/db-0";
        const LOG_PATH: &str = "/api/v1/namespaces/shop/pods/db-0/log";
        const WAIT: Duration = Duration::from_secs(10);

        fn at(second: u32, nanos: u32, text: &str) -> String {
            format!("2026-01-01T00:00:{second:02}.{nanos:09}Z {text}")
        }

        fn nanos(line: &str) -> i64 {
            split_timestamp(line.as_bytes()).0.unwrap()
        }

        /// How the fake kubelet answers one log request.
        #[derive(Clone)]
        enum Log {
            /// The lines it holds. `cut` drops the connection after sending them.
            Holds {
                lines: Vec<String>,
                cut: bool,
            },
            Fails(u16),
        }

        /// How the fake API answers a request for the pod.
        #[derive(Clone)]
        enum PodState {
            Running {
                restarts: i32,
                started: &'static str,
            },
            Gone,
        }

        /// Answers each request with the next step, and repeats the last one.
        #[derive(Default)]
        struct Script {
            logs: Vec<Log>,
            previous: Option<Log>,
            pods: Vec<PodState>,
            log_requests: Vec<Request>,
            pod_requests: usize,
        }

        fn step<T: Clone>(steps: &[T], index: usize) -> T {
            steps[index.min(steps.len() - 1)].clone()
        }

        /// Applies `tailLines`, then `sinceTime`, as the kubelet does.
        fn kubelet(lines: &[String], request: &Request) -> String {
            let tail = request
                .query
                .get("tailLines")
                .map_or(usize::MAX, |tail| tail.parse().unwrap());
            let since = request
                .query
                .get("sinceTime")
                .map(|since| parse_rfc3339(since.as_bytes()).unwrap());
            lines[lines.len().saturating_sub(tail)..]
                .iter()
                .filter(|line| since.is_none_or(|since| nanos(line) >= since))
                .map(|line| format!("{line}\n"))
                .collect()
        }

        fn answer(log: Log, request: &Request) -> Reply {
            match log {
                Log::Holds { lines, cut: true } => Reply::Cut(kubelet(&lines, request)),
                Log::Holds { lines, cut: false } => Reply::Text(kubelet(&lines, request)),
                Log::Fails(code) => Reply::Status(code),
            }
        }

        fn pod_reply(state: PodState) -> Reply {
            match state {
                PodState::Running { restarts, started } => Reply::Json(format!(
                    r#"{{"apiVersion":"v1","kind":"Pod","metadata":{{"name":"db-0","namespace":"shop","uid":"uid-db-0"}},"status":{{"containerStatuses":[{{"name":"app","image":"app","imageID":"","ready":true,"restartCount":{restarts},"containerID":"containerd://run-{restarts}","state":{{"running":{{"startedAt":"{started}"}}}}}}]}}}}"#
                )),
                PodState::Gone => Reply::Status(404),
            }
        }

        fn serve(script: &Mutex<Script>, request: &Request) -> Option<Reply> {
            let mut script = script.lock().unwrap();
            if request.path == POD_PATH {
                let state = step(&script.pods, script.pod_requests);
                script.pod_requests += 1;
                return Some(pod_reply(state));
            }
            if request.path != LOG_PATH {
                return None;
            }
            if request
                .query
                .get("previous")
                .is_some_and(|value| value == "true")
            {
                let log = script.previous.clone().unwrap_or(Log::Fails(400));
                return Some(answer(log, request));
            }
            let log = step(&script.logs, script.log_requests.len());
            script.log_requests.push(request.clone());
            Some(answer(log, request))
        }

        fn running(restarts: i32, started: &'static str) -> PodState {
            PodState::Running { restarts, started }
        }

        fn holds(lines: &[String], cut: bool) -> Log {
            Log::Holds {
                lines: lines.to_vec(),
                cut,
            }
        }

        /// Tails `db-0/app` until `done`, then returns its lines, gaps as
        /// `gap <from> <to>`, and leaves out Taffrail's other notes.
        async fn tail_until(
            script: Script,
            done: impl Fn(&Script, &[LogLine]) -> bool,
        ) -> (Vec<String>, Arc<Mutex<Script>>) {
            let script = Arc::new(Mutex::new(script));
            let handler = {
                let script = script.clone();
                Arc::new(move |request: &Request| serve(&script, request))
            };
            let connection = fake_api::connect_with(handler).await;
            let tail = start(
                connection.client,
                vec![LogTarget {
                    namespace: "shop".into(),
                    pod: "db-0".into(),
                    container: "app".into(),
                }],
            )
            .unwrap();
            let reader = tail.reader();
            let stored = tokio::time::timeout(WAIT, async {
                loop {
                    let stored = reader.read(0..u64::MAX).unwrap();
                    if done(&script.lock().unwrap(), &stored) {
                        return stored;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the tail never got there");
            let lines = stored
                .into_iter()
                .filter_map(|line| match line.gap {
                    Some(gap) => Some(format!("gap {} {}", gap.from, gap.to)),
                    None => line.timestamp.map(|_| line.text),
                })
                .collect();
            (lines, script)
        }

        /// Done once the stream has reconnected `extra` times past the script, so any
        /// duplicate would have arrived.
        fn settled(extra: usize) -> impl Fn(&Script, &[LogLine]) -> bool {
            move |script, _| script.log_requests.len() >= script.logs.len() + extra
        }

        fn gap(from: &str, to: &str) -> String {
            format!("gap {} {}", nanos(from), nanos(to))
        }

        #[tokio::test]
        async fn a_dropped_stream_resumes_without_loss_or_duplicates() {
            let log = [
                at(1, 100, "a"),
                at(1, 200, "b"),
                at(2, 0, "c"),
                at(2, 500, "d"),
                at(2, 900, "e"),
                at(3, 0, "f"),
                at(4, 0, "g"),
            ];
            let script = Script {
                logs: vec![
                    holds(&log[..4], true),
                    holds(&log[..6], false),
                    holds(&log, false),
                ],
                pods: vec![running(0, "2026-01-01T00:00:00Z")],
                ..Script::default()
            };

            let (lines, script) = tail_until(script, settled(3)).await;

            assert_eq!(lines, log);
            let requests = &script.lock().unwrap().log_requests;
            assert_eq!(requests[0].query.get("tailLines").unwrap(), "100");
            assert_eq!(requests[0].query.get("sinceTime"), None);
            assert_eq!(
                requests[1].query.get("sinceTime").unwrap(),
                "2026-01-01T00:00:02Z",
                "a reconnect asks from the second of the last line"
            );
            assert_eq!(requests[1].query.get("tailLines").unwrap(), "10000");
            assert_eq!(
                requests[2].query.get("sinceTime").unwrap(),
                "2026-01-01T00:00:03Z"
            );
        }

        #[tokio::test]
        async fn lines_with_the_same_timestamp_are_kept_once_each() {
            let log = [
                at(5, 0, "x"),
                at(5, 0, "x"),
                at(5, 0, "y"),
                at(5, 0, "x"),
                at(6, 0, "z"),
            ];
            let script = Script {
                logs: vec![holds(&log[..2], true), holds(&log, false)],
                pods: vec![running(0, "2026-01-01T00:00:00Z")],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, settled(3)).await;

            assert_eq!(lines, log);
        }

        #[tokio::test]
        async fn a_restarted_container_reads_its_previous_run_then_the_new_one() {
            let old = [at(1, 0, "old 1"), at(2, 0, "old 2"), at(3, 0, "old 3")];
            let new = [at(4, 0, "new 1"), at(5, 0, "new 2")];
            let script = Script {
                logs: vec![holds(&old[..1], true), holds(&new, false)],
                previous: Some(holds(&old, false)),
                pods: vec![
                    running(0, "2026-01-01T00:00:00Z"),
                    running(1, "2026-01-01T00:00:04Z"),
                ],
                ..Script::default()
            };

            let (lines, script) = tail_until(script, settled(3)).await;

            assert_eq!(lines, [old.as_slice(), new.as_slice()].concat());
            let requests = &script.lock().unwrap().log_requests;
            assert_eq!(
                requests[1].query.get("sinceTime"),
                None,
                "the new run is read from its start"
            );
            assert_eq!(
                requests[2].query.get("sinceTime").unwrap(),
                "2026-01-01T00:00:05Z"
            );
        }

        #[tokio::test]
        async fn a_restart_with_no_previous_log_marks_the_gap() {
            let old = [at(1, 0, "old 1"), at(2, 0, "old 2")];
            let new = [at(4, 0, "new 1"), at(5, 0, "new 2")];
            let script = Script {
                logs: vec![holds(&old[..1], true), holds(&new, false)],
                previous: Some(Log::Fails(400)),
                pods: vec![
                    running(0, "2026-01-01T00:00:00Z"),
                    running(1, "2026-01-01T00:00:03.5Z"),
                ],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, settled(3)).await;

            assert_eq!(
                lines,
                [
                    old[0].clone(),
                    gap(&old[0], "2026-01-01T00:00:03.5Z x"),
                    new[0].clone(),
                    new[1].clone(),
                ]
            );
        }

        #[tokio::test]
        async fn lines_that_cannot_be_read_again_are_marked_missing() {
            let log = [
                at(1, 0, "a"),
                at(2, 0, "b"),
                at(3, 0, "c"),
                at(4, 0, "d"),
                at(5, 0, "e"),
            ];
            // The log rotated while the stream was down, so only the newest lines remain.
            let script = Script {
                logs: vec![holds(&log[..2], true), holds(&log[3..], false)],
                pods: vec![running(0, "2026-01-01T00:00:00Z")],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, settled(3)).await;

            assert_eq!(
                lines,
                [
                    log[0].clone(),
                    log[1].clone(),
                    gap(&log[1], &log[3]),
                    log[3].clone(),
                    log[4].clone(),
                ]
            );
        }

        #[tokio::test]
        async fn a_reconnect_that_keeps_failing_marks_the_gap_when_it_returns() {
            let log = [at(1, 0, "a"), at(2, 0, "b"), at(9, 0, "c")];
            let script = Script {
                logs: vec![
                    holds(&log[..1], true),
                    Log::Fails(503),
                    Log::Fails(503),
                    holds(&log[2..], false),
                ],
                pods: vec![running(0, "2026-01-01T00:00:00Z")],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, settled(3)).await;

            assert_eq!(
                lines,
                [log[0].clone(), gap(&log[0], &log[2]), log[2].clone()]
            );
        }

        fn has_note(stored: &[LogLine], text: &str) -> bool {
            stored
                .iter()
                .any(|line| line.timestamp.is_none() && line.text.contains(text))
        }

        #[tokio::test]
        async fn a_pod_gone_after_a_lost_connection_marks_the_rest_missing() {
            let log = [at(1, 0, "a"), at(2, 0, "b")];
            let script = Script {
                logs: vec![holds(&log, true)],
                pods: vec![running(0, "2026-01-01T00:00:00Z"), PodState::Gone],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, |_, stored| has_note(stored, "404")).await;

            assert_eq!(lines.len(), 3);
            assert_eq!(lines[..2], log);
            let (from, to) = lines[2]
                .strip_prefix("gap ")
                .and_then(|range| range.split_once(' '))
                .unwrap();
            assert_eq!(from.parse::<i64>().unwrap(), nanos(&log[1]));
            assert!(to.parse::<i64>().unwrap() > nanos(&log[1]));
        }

        #[tokio::test]
        async fn a_pod_gone_after_its_log_ended_has_no_gap() {
            let log = [at(1, 0, "a"), at(2, 0, "b")];
            let script = Script {
                logs: vec![holds(&log, false)],
                pods: vec![running(0, "2026-01-01T00:00:00Z"), PodState::Gone],
                ..Script::default()
            };

            let (lines, _) = tail_until(script, |_, stored| has_note(stored, "404")).await;

            assert_eq!(lines, log);
        }

        #[tokio::test]
        async fn requests_name_the_container_and_ask_for_timestamps() {
            let script = Script {
                logs: vec![holds(&[at(1, 0, "a")], false)],
                pods: vec![running(0, "2026-01-01T00:00:00Z")],
                ..Script::default()
            };

            let (_, script) = tail_until(script, settled(1)).await;

            let query: &HashMap<String, String> = &script.lock().unwrap().log_requests[0].query;
            assert_eq!(query.get("container").unwrap(), "app");
            assert_eq!(query.get("timestamps").unwrap(), "true");
            assert_eq!(query.get("follow").unwrap(), "true");
        }
    }

    /// Tails 50 simulated pods through a proxy that cuts every connection at an interval,
    /// then checks each source's `seq` numbers for lost and repeated lines. Run with
    /// `TAFFRAIL_SIM_CONTEXT=kwok-taffrail cargo test --release -p taffrail-core sim_reconnects -- --ignored --nocapture`.
    mod sim {
        use std::env;
        use std::process::Command;

        use kube::api::ListParams;
        use tokio::net::{TcpListener, TcpStream};

        use super::*;
        use crate::cluster::Contexts;

        fn setting(name: &str, default: u64) -> u64 {
            env::var(name).map_or(default, |value| value.parse().unwrap())
        }

        /// Forwards to `upstream`, and cuts every open connection each `cut_every`.
        async fn cutting_proxy(upstream: String, cut_every: Duration) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let open = Arc::new(Mutex::new(Vec::<AbortHandle>::new()));
            {
                let open = open.clone();
                tokio::spawn(async move {
                    while let Ok((mut client, _)) = listener.accept().await {
                        let upstream = upstream.clone();
                        let task = tokio::spawn(async move {
                            // A refused connection closes the client's, as a server that is down.
                            let Ok(mut server) = TcpStream::connect(upstream).await else {
                                return;
                            };
                            tokio::io::copy_bidirectional(&mut client, &mut server)
                                .await
                                .ok();
                        });
                        open.lock().unwrap().push(task.abort_handle());
                    }
                });
            }
            if !cut_every.is_zero() {
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(cut_every).await;
                        for connection in open.lock().unwrap().drain(..) {
                            connection.abort();
                        }
                    }
                });
            }
            address
        }

        fn seq(text: &str) -> Option<u64> {
            let rest = &text[text.find("\"seq\": ")? + "\"seq\": ".len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            rest[..digits].parse().ok()
        }

        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "a measurement against a simulated cluster"]
        async fn sim_reconnects() {
            let context = env::var("TAFFRAIL_SIM_CONTEXT").expect("set TAFFRAIL_SIM_CONTEXT");
            assert!(context.starts_with("kwok-"), "only a simulated cluster");
            let seconds = setting("TAFFRAIL_SIM_SECONDS", 60);
            let cut_every = Duration::from_secs(setting("TAFFRAIL_SIM_CUT_SECONDS", 10));
            let output = Command::new("kubectl")
                .args(["config", "view", "--raw", "--minify", "--flatten"])
                .arg(format!("--context={context}"))
                .output()
                .unwrap();
            let kubeconfig = String::from_utf8(output.stdout).unwrap();
            let server = kubeconfig
                .lines()
                .find_map(|line| line.trim().strip_prefix("server: https://"))
                .unwrap()
                .to_owned();
            let proxy = cutting_proxy(server.clone(), cut_every).await;
            let kubeconfig = kubeconfig.replace(&server, &proxy);
            let connection = Contexts::from_yaml(&kubeconfig)
                .unwrap()
                .connect(context)
                .await
                .unwrap();
            let pods: Api<Pod> = Api::default_namespaced(connection.client.clone());
            let targets: Vec<LogTarget> = pods
                .list(&ListParams::default().limit(MAX_TAIL_PODS as u32))
                .await
                .unwrap()
                .items
                .into_iter()
                .take(MAX_TAIL_PODS)
                .map(|pod| LogTarget {
                    namespace: "default".into(),
                    pod: pod.metadata.name.unwrap(),
                    container: pod.spec.unwrap().containers[0].name.clone(),
                })
                .collect();
            let sources = targets.len();
            let tail = start(connection.client, targets).unwrap();
            tokio::time::sleep(Duration::from_secs(seconds)).await;

            let reader = tail.reader();
            let stored = reader.read(0..u64::MAX).unwrap();
            let mut last: HashMap<u16, u64> = HashMap::new();
            let (mut lines, mut lost, mut repeated, mut gaps, mut notes) = (0u64, 0, 0, 0, 0);
            for line in &stored {
                if line.gap.is_some() {
                    gaps += 1;
                    continue;
                }
                let Some(seq) = seq(&line.text) else {
                    notes += 1;
                    continue;
                };
                lines += 1;
                if let Some(previous) = last.insert(line.source, seq) {
                    if seq <= previous {
                        repeated += 1;
                    } else {
                        lost += seq - previous - 1;
                    }
                }
            }
            let newest = last.values().max().copied().unwrap_or(0);
            let behind = last.values().map(|seq| newest - seq).max().unwrap_or(0);
            println!(
                r#"{{"sources":{sources},"seconds":{seconds},"cut_every_s":{},"lines":{lines},"lines_per_second":{:.0},"lost":{lost},"repeated":{repeated},"gaps":{gaps},"notes":{notes},"most_behind":{behind}}}"#,
                cut_every.as_secs(),
                lines as f64 / seconds as f64,
            );
        }
    }

    #[tokio::test]
    async fn the_writer_stops_when_senders_are_gone() {
        let (lines, _store, progress, writer) = writer();

        drop(lines);
        writer.join().unwrap();

        assert_eq!(progress.borrow().end, 0);
    }
}
