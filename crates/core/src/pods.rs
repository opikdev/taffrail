use std::collections::BTreeMap;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::WatchStreamExt;
use kube::runtime::watcher::{self, Event};
use kube::{Api, Client};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::logs::{LogTarget, MAX_TAIL_PODS};

const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

pub type PodKey = (String, String);
type PodRows = BTreeMap<PodKey, Arc<PodRow>>;

/// The fields of a pod that the table shows, so full objects need not stay in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodRow {
    pub namespace: String,
    pub name: String,
    pub status: String,
    pub ready: String,
    pub restarts: i32,
    pub node: String,
    pub created_unix: Option<i64>,
    pub containers: Vec<String>,
}

impl PodRow {
    /// What identifies this pod across updates, as its row moves or changes.
    pub fn key(&self) -> PodKey {
        (self.namespace.clone(), self.name.clone())
    }

    /// Whether `filter` appears in `namespace/name`, ignoring case.
    pub fn matches(&self, filter: &str) -> bool {
        format!("{}/{}", self.namespace, self.name)
            .to_lowercase()
            .contains(&filter.to_lowercase())
    }
}

impl From<&Pod> for PodRow {
    fn from(pod: &Pod) -> Self {
        let containers = pod
            .status
            .as_ref()
            .and_then(|status| status.container_statuses.as_deref())
            .unwrap_or_default();
        let total = pod.spec.as_ref().map_or(0, |spec| spec.containers.len());
        let ready = containers
            .iter()
            .filter(|container| container.ready)
            .count();
        Self {
            namespace: pod.metadata.namespace.clone().unwrap_or_default(),
            name: pod.metadata.name.clone().unwrap_or_default(),
            status: display_status(pod),
            ready: format!("{ready}/{total}"),
            restarts: containers
                .iter()
                .map(|container| container.restart_count)
                .sum(),
            node: pod
                .spec
                .as_ref()
                .and_then(|spec| spec.node_name.clone())
                .unwrap_or_default(),
            created_unix: pod
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|created| created.0.as_second()),
            containers: pod
                .spec
                .iter()
                .flat_map(|spec| &spec.containers)
                .map(|container| container.name.clone())
                .collect(),
        }
    }
}

fn display_status(pod: &Pod) -> String {
    if pod.metadata.deletion_timestamp.is_some() {
        return "Terminating".to_owned();
    }
    let status = pod.status.as_ref();
    status
        .and_then(|status| status.container_statuses.as_deref())
        .unwrap_or_default()
        .iter()
        .find_map(|container| {
            let state = container.state.as_ref()?;
            let waiting = state
                .waiting
                .as_ref()
                .and_then(|waiting| waiting.reason.clone());
            waiting.or_else(|| state.terminated.as_ref()?.reason.clone())
        })
        .or_else(|| status?.reason.clone())
        .or_else(|| status?.phase.clone())
        .unwrap_or_else(|| "Unknown".to_owned())
}

/// Pods at one moment, sorted by namespace and name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodSnapshot {
    pub rows: Vec<Arc<PodRow>>,
    pub synced: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct PodState {
    rows: PodRows,
    relisting: Option<PodRows>,
    synced: bool,
    error: Option<String>,
}

impl PodSnapshot {
    /// The row for `key`, if the pod still exists.
    pub fn find(&self, key: &PodKey) -> Option<&Arc<PodRow>> {
        let (namespace, name) = key;
        self.rows
            .binary_search_by(|row| {
                (row.namespace.as_str(), row.name.as_str())
                    .cmp(&(namespace.as_str(), name.as_str()))
            })
            .ok()
            .map(|ix| &self.rows[ix])
    }
}

/// Log targets for the first `MAX_TAIL_PODS` of `rows`.
pub fn log_targets<'a>(rows: impl IntoIterator<Item = &'a Arc<PodRow>>) -> Vec<LogTarget> {
    rows.into_iter()
        .take(MAX_TAIL_PODS)
        .flat_map(|row| {
            row.containers.iter().map(|container| LogTarget {
                namespace: row.namespace.clone(),
                pod: row.name.clone(),
                container: container.clone(),
            })
        })
        .collect()
}

impl PodState {
    fn record(&mut self, event: watcher::Result<Event<Pod>>) {
        match event {
            Ok(event) => {
                self.error = None;
                self.apply(event);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn apply(&mut self, event: Event<Pod>) {
        match event {
            Event::Apply(pod) => {
                let row = PodRow::from(&pod);
                self.rows.insert(row.key(), Arc::new(row));
            }
            Event::Delete(pod) => {
                self.rows.remove(&PodRow::from(&pod).key());
            }
            Event::Init => self.relisting = Some(PodRows::new()),
            Event::InitApply(pod) => {
                let row = PodRow::from(&pod);
                self.relisting
                    .get_or_insert_default()
                    .insert(row.key(), Arc::new(row));
            }
            Event::InitDone => {
                self.rows = self.relisting.take().unwrap_or_default();
                self.synced = true;
            }
        }
    }

    fn snapshot(&self) -> PodSnapshot {
        PodSnapshot {
            rows: self.rows.values().cloned().collect(),
            synced: self.synced,
            error: self.error.clone(),
        }
    }
}

/// Publishes at most once per interval, and not at all while the stream is quiet.
async fn publish(
    events: impl Stream<Item = watcher::Result<Event<Pod>>>,
    snapshots: watch::Sender<Arc<PodSnapshot>>,
) {
    let mut events = pin!(events);
    let mut state = PodState::default();
    while let Some(event) = events.next().await {
        state.record(event);
        let mut deadline = pin!(tokio::time::sleep(PUBLISH_INTERVAL));
        let mut ended = false;
        while !ended {
            tokio::select! {
                () = &mut deadline => break,
                event = events.next() => match event {
                    Some(event) => state.record(event),
                    None => ended = true,
                },
            }
        }
        if snapshots.send(Arc::new(state.snapshot())).is_err() {
            return;
        }
    }
}

/// A running pod watch. Dropping it stops the watch.
pub struct PodWatch {
    snapshots: watch::Receiver<Arc<PodSnapshot>>,
    task: JoinHandle<()>,
}

impl PodWatch {
    /// Waits for the newest snapshot, or returns `None` if the watch has ended.
    pub async fn next(&mut self) -> Option<Arc<PodSnapshot>> {
        self.snapshots.changed().await.ok()?;
        Some(self.snapshots.borrow_and_update().clone())
    }
}

impl Drop for PodWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn watch(client: Client) -> PodWatch {
    let (sender, snapshots) = watch::channel(Arc::default());
    let events = watcher::watcher(Api::<Pod>::all(client), watcher::Config::default());
    let task = tokio::spawn(publish(events.default_backoff(), sender));
    PodWatch { snapshots, task }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn channel_stream<T>(receiver: mpsc::UnboundedReceiver<T>) -> impl Stream<Item = T> {
        stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|item| (item, receiver))
        })
    }

    fn pod(namespace: &str, name: &str) -> Pod {
        serde_json::from_value(json!({
            "metadata": { "namespace": namespace, "name": name },
        }))
        .unwrap()
    }

    fn names(snapshot: &PodSnapshot) -> Vec<&str> {
        snapshot.rows.iter().map(|row| row.name.as_str()).collect()
    }

    fn listed(pods: Vec<Pod>) -> PodState {
        let mut state = PodState::default();
        state.apply(Event::Init);
        for pod in pods {
            state.apply(Event::InitApply(pod));
        }
        state.apply(Event::InitDone);
        state
    }

    #[test]
    fn row_summarizes_a_running_pod() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "api-1",
                "creationTimestamp": "2026-01-01T00:00:00Z",
            },
            "spec": {
                "nodeName": "node-1",
                "containers": [{ "name": "app" }, { "name": "proxy" }],
            },
            "status": {
                "phase": "Running",
                "containerStatuses": [
                    { "name": "app", "image": "", "imageID": "", "ready": true, "restartCount": 2 },
                    { "name": "proxy", "image": "", "imageID": "", "ready": false, "restartCount": 3 },
                ],
            },
        }))
        .unwrap();

        assert_eq!(
            PodRow::from(&pod),
            PodRow {
                namespace: "shop".into(),
                name: "api-1".into(),
                status: "Running".into(),
                ready: "1/2".into(),
                restarts: 5,
                node: "node-1".into(),
                created_unix: Some(1_767_225_600),
                containers: vec!["app".into(), "proxy".into()],
            }
        );
    }

    #[test]
    fn status_prefers_the_container_reason_over_the_phase() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "api" },
            "status": {
                "phase": "Running",
                "containerStatuses": [{
                    "name": "app", "image": "", "imageID": "", "ready": false, "restartCount": 9,
                    "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                }],
            },
        }))
        .unwrap();

        assert_eq!(PodRow::from(&pod).status, "CrashLoopBackOff");
    }

    #[test]
    fn status_shows_terminating_for_a_pod_being_deleted() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "api", "deletionTimestamp": "2026-01-01T00:00:00Z" },
            "status": { "phase": "Running" },
        }))
        .unwrap();

        assert_eq!(PodRow::from(&pod).status, "Terminating");
    }

    #[test]
    fn status_is_unknown_without_a_pod_status() {
        assert_eq!(PodRow::from(&pod("shop", "api")).status, "Unknown");
    }

    fn pod_with_containers(name: &str, containers: &[&str]) -> Pod {
        let containers: Vec<_> = containers
            .iter()
            .map(|name| json!({ "name": name }))
            .collect();
        serde_json::from_value(json!({
            "metadata": { "namespace": "shop", "name": name },
            "spec": { "containers": containers },
        }))
        .unwrap()
    }

    #[test]
    fn matches_namespace_and_name_ignoring_case() {
        let row = PodRow::from(&pod("Shop", "API-1"));

        assert!(row.matches(""));
        assert!(row.matches("shop/api"));
        assert!(row.matches("api-1"));
        assert!(!row.matches("web"));
    }

    #[test]
    fn log_targets_cover_every_container_in_the_order_given() {
        let snapshot = listed(vec![
            pod_with_containers("api", &["app", "proxy"]),
            pod_with_containers("web", &["app"]),
        ])
        .snapshot();

        let targets = log_targets(snapshot.rows.iter().rev());

        let names: Vec<_> = targets
            .iter()
            .map(|t| (t.pod.as_str(), t.container.as_str()))
            .collect();
        assert_eq!(names, [("web", "app"), ("api", "app"), ("api", "proxy")]);
    }

    #[test]
    fn find_locates_a_pod_by_namespace_and_name() {
        let snapshot = listed(vec![pod("b", "api"), pod("a", "web"), pod("a", "api")]).snapshot();
        let key = ("a".to_owned(), "web".to_owned());
        assert_eq!(snapshot.find(&key).unwrap().name, "web");
        assert!(snapshot.find(&("c".to_owned(), "api".to_owned())).is_none());
    }

    #[test]
    fn log_targets_stop_at_the_pod_limit() {
        let pods = (0..MAX_TAIL_PODS + 5)
            .map(|index| pod_with_containers(&format!("pod-{index:03}"), &["app"]))
            .collect();

        let targets = log_targets(&listed(pods).snapshot().rows);

        assert_eq!(targets.len(), MAX_TAIL_PODS);
    }

    #[test]
    fn rows_stay_hidden_until_the_list_finishes() {
        let mut state = PodState::default();
        state.apply(Event::Init);
        state.apply(Event::InitApply(pod("shop", "api")));

        let snapshot = state.snapshot();

        assert!(snapshot.rows.is_empty());
        assert!(!snapshot.synced);
    }

    #[test]
    fn rows_sort_by_namespace_then_name() {
        let state = listed(vec![pod("b", "a"), pod("a", "z"), pod("a", "b")]);

        let snapshot = state.snapshot();
        let keys: Vec<_> = snapshot
            .rows
            .iter()
            .map(|row| (row.namespace.as_str(), row.name.as_str()))
            .collect();

        assert_eq!(keys, [("a", "b"), ("a", "z"), ("b", "a")]);
        assert!(snapshot.synced);
    }

    #[test]
    fn apply_and_delete_update_rows() {
        let mut state = listed(vec![pod("a", "old")]);

        state.apply(Event::Apply(pod("a", "new")));
        state.apply(Event::Delete(pod("a", "old")));

        assert_eq!(names(&state.snapshot()), ["new"]);
    }

    #[test]
    fn a_relist_drops_pods_deleted_during_a_disconnect() {
        let mut state = listed(vec![pod("a", "gone"), pod("a", "kept")]);

        state.apply(Event::Init);
        state.apply(Event::InitApply(pod("a", "kept")));
        assert_eq!(names(&state.snapshot()), ["gone", "kept"]);
        state.apply(Event::InitDone);

        assert_eq!(names(&state.snapshot()), ["kept"]);
    }

    #[test]
    fn an_error_keeps_rows_and_clears_on_the_next_event() {
        let mut state = listed(vec![pod("a", "api")]);

        state.record(Err(watcher::Error::NoResourceVersion));
        let failed = state.snapshot();
        state.record(Ok(Event::Apply(pod("a", "web"))));
        let recovered = state.snapshot();

        assert_eq!(names(&failed), ["api"]);
        assert!(failed.error.is_some());
        assert_eq!(recovered.error, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_events_publishes_one_snapshot() {
        let (events, received) = mpsc::unbounded_channel();
        let (sender, mut snapshots) = watch::channel(Arc::default());
        let task = tokio::spawn(publish(channel_stream(received), sender));

        events.send(Ok(Event::Init)).unwrap();
        for index in 0..1000 {
            events
                .send(Ok(Event::InitApply(pod("a", &format!("pod-{index}")))))
                .unwrap();
        }
        events.send(Ok(Event::InitDone)).unwrap();
        tokio::time::sleep(PUBLISH_INTERVAL * 2).await;

        assert!(snapshots.has_changed().unwrap());
        assert_eq!(snapshots.borrow_and_update().rows.len(), 1000);

        tokio::time::sleep(PUBLISH_INTERVAL * 10).await;

        assert!(
            !snapshots.has_changed().unwrap(),
            "idle stream must not publish"
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn publishes_the_final_events_when_the_stream_ends() {
        let events = stream::iter(vec![Ok(Event::Init), Ok(Event::InitDone)]);
        let (sender, mut snapshots) = watch::channel(Arc::default());

        publish(events, sender).await;

        assert!(snapshots.borrow_and_update().synced);
    }

    #[tokio::test]
    async fn dropping_a_watch_stops_its_task() {
        let (sender, snapshots) = watch::channel(Arc::default());
        let task = tokio::spawn(async move {
            sender.closed().await;
        });
        let abort = task.abort_handle();

        drop(PodWatch { snapshots, task });
        tokio::task::yield_now().await;

        assert!(abort.is_finished());
    }
}
