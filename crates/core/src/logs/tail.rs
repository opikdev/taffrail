use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::Range;
use std::pin::pin;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{AsyncBufRead, AsyncBufReadExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::LogParams;
use kube::{Api, Client};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

use super::store::{LogLine, LogStore, Record};
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
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
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
    targets: Arc<RwLock<Vec<LogTarget>>>,
    store: Arc<Mutex<LogStore>>,
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

/// Follows one container until it is gone. `from_start` reads its log from the first
/// line, for a pod that started after the tail did.
async fn follow_target(
    pods: Api<Pod>,
    source: u16,
    target: LogTarget,
    from_start: bool,
    lines: mpsc::Sender<Record>,
) {
    let mut params = LogParams {
        container: Some(target.container.clone()),
        follow: true,
        timestamps: true,
        tail_lines: Some(if from_start {
            NEW_POD_TAIL_LINES
        } else {
            INITIAL_TAIL_LINES
        }),
        ..LogParams::default()
    };
    let mut last_note = None;
    loop {
        let mut opened = false;
        let ended = follow_stream(&pods, &target.pod, &params, source, &lines, &mut opened).await;
        let not_found = matches!(&ended, Err(kube::Error::Api(status)) if status.code == NOT_FOUND);
        let reason = match ended {
            Ok(()) => "tail ended".to_owned(),
            Err(error) => error.to_string(),
        };
        // A pod that is still starting fails the same way until it runs, so say it once.
        let note = format!("[taffrail: {reason}]");
        if last_note.as_ref() != Some(&note) {
            if lines
                .send(Record::note(source, note.clone().into_bytes()))
                .await
                .is_err()
            {
                return;
            }
            last_note = Some(note);
        }
        if not_found {
            return;
        }
        if opened {
            // Earlier lines are already stored, so a reconnect only wants new ones.
            params.tail_lines = Some(0);
            last_note = None;
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn follow_stream(
    pods: &Api<Pod>,
    pod: &str,
    params: &LogParams,
    source: u16,
    lines: &mpsc::Sender<Record>,
    opened: &mut bool,
) -> Result<(), kube::Error> {
    let stream = pods.log_stream(pod, params).await?;
    *opened = true;
    let mut stream = pin!(stream);
    let mut line = Vec::new();
    loop {
        match read_line(&mut stream, &mut line).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(error) => return Err(kube::Error::ReadEvents(error)),
        }
        // Each stream finds its own lines' severity, so the work spreads across the
        // runtime's threads and stays out of the store's lock.
        if lines
            .send(Record::new(source, std::mem::take(&mut line)))
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

    #[tokio::test]
    async fn the_writer_stops_when_senders_are_gone() {
        let (lines, _store, progress, writer) = writer();

        drop(lines);
        writer.join().unwrap();

        assert_eq!(progress.borrow().end, 0);
    }
}
