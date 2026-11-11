//! A live watch of one kind of object, shared by every resource view.
//!
//! A watch lists and watches across all namespaces, and the views narrow its rows on the
//! client. If the API refuses that list, as it does for a role bound in one namespace, the
//! watch falls back to the namespace the user has chosen, and moves with that choice.

use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use kube::runtime::WatchStreamExt;
use kube::runtime::watcher::{self, Event};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
const FORBIDDEN: u16 = 403;
const NOT_FOUND: u16 = 404;

/// What identifies an object across updates: its namespace, empty for a cluster-scoped
/// kind, and its name.
pub type ObjectKey = (String, String);

/// The fields of one kind of object that the app shows, so full objects need not stay in
/// memory. Each kind of resource view has one.
pub trait Row: Clone + Debug + PartialEq + Send + Sync + 'static {
    type Object: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug + Send + 'static;

    /// Whether objects live in namespaces, so that a view narrows them to the chosen one.
    const NAMESPACED: bool = true;

    fn from_object(object: &Self::Object) -> Self;

    /// The objects in `namespace`, or in every namespace with `None`. A cluster-scoped kind
    /// ignores `namespace`.
    fn api(client: Client, namespace: Option<&str>) -> Api<Self::Object>;

    fn namespace(&self) -> &str;

    fn name(&self) -> &str;

    fn key(&self) -> ObjectKey {
        (self.namespace().to_owned(), self.name().to_owned())
    }

    /// Whether `filter`, already in lowercase, appears in `namespace/name`, ignoring case.
    /// An empty filter matches without allocating, because views rebuild their rows on
    /// every snapshot.
    fn matches(&self, filter: &str) -> bool {
        filter.is_empty()
            || format!("{}/{}", self.namespace(), self.name())
                .to_lowercase()
                .contains(filter)
    }
}

/// An API for a namespaced kind, in `namespace` or in all of them.
pub fn namespaced_api<K>(client: Client, namespace: Option<&str>) -> Api<K>
where
    K: Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>,
{
    match namespace {
        Some(namespace) => Api::namespaced(client, namespace),
        None => Api::all(client),
    }
}

/// Why a watch has no current rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchError {
    /// The API refused to list or watch, in `namespace`, or across all namespaces with
    /// `None`. `message` is the API's reason.
    Forbidden {
        namespace: Option<String>,
        message: String,
    },
    /// The cluster does not serve this kind of object.
    NotServed { message: String },
    /// Anything else, such as a lost connection. The watch keeps retrying.
    Failed { message: String },
}

impl WatchError {
    fn from_watcher(error: &watcher::Error, namespace: Option<&str>) -> Self {
        let status = match error {
            watcher::Error::InitialListFailed(kube::Error::Api(status))
            | watcher::Error::WatchStartFailed(kube::Error::Api(status))
            | watcher::Error::WatchFailed(kube::Error::Api(status))
            | watcher::Error::WatchError(status) => Some(status),
            _ => None,
        };
        match status {
            Some(status) if status.code == FORBIDDEN => Self::Forbidden {
                namespace: namespace.map(str::to_owned),
                message: status.message.clone(),
            },
            Some(status) if status.code == NOT_FOUND => Self::NotServed {
                message: status.message.clone(),
            },
            _ => Self::Failed {
                message: error.to_string(),
            },
        }
    }

    /// Whether this is a refusal, rather than a failure that retrying may fix.
    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }
}

impl Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forbidden { message, .. }
            | Self::NotServed { message }
            | Self::Failed { message } => f.write_str(message),
        }
    }
}

/// Objects of one kind at one moment, sorted by namespace and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot<R> {
    pub rows: Vec<Arc<R>>,
    pub synced: bool,
    pub error: Option<WatchError>,
}

impl<R> Default for Snapshot<R> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            synced: false,
            error: None,
        }
    }
}

impl<R: Row> Snapshot<R> {
    /// The row for `key`, if the object still exists.
    pub fn find(&self, key: &ObjectKey) -> Option<&Arc<R>> {
        let (namespace, name) = key;
        self.rows
            .binary_search_by(|row| (row.namespace(), row.name()).cmp(&(namespace, name)))
            .ok()
            .map(|ix| &self.rows[ix])
    }
}

type Rows<R> = BTreeMap<ObjectKey, Arc<R>>;

struct State<R> {
    rows: Rows<R>,
    relisting: Option<Rows<R>>,
    synced: bool,
    error: Option<WatchError>,
}

impl<R> Default for State<R> {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
            relisting: None,
            synced: false,
            error: None,
        }
    }
}

impl<R: Row> State<R> {
    fn record(&mut self, event: watcher::Result<Event<R::Object>>, namespace: Option<&str>) {
        match event {
            Ok(event) => {
                self.error = None;
                self.apply(event);
            }
            Err(error) => self.error = Some(WatchError::from_watcher(&error, namespace)),
        }
    }

    fn apply(&mut self, event: Event<R::Object>) {
        match event {
            Event::Apply(object) => {
                let row = R::from_object(&object);
                self.rows.insert(row.key(), Arc::new(row));
            }
            Event::Delete(object) => {
                self.rows.remove(&R::from_object(&object).key());
            }
            Event::Init => self.relisting = Some(Rows::new()),
            Event::InitApply(object) => {
                let row = R::from_object(&object);
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

    fn snapshot(&self) -> Snapshot<R> {
        Snapshot {
            rows: self.rows.values().cloned().collect(),
            synced: self.synced,
            error: self.error.clone(),
        }
    }
}

/// Where a watch looks: across all namespaces, unless the API refused that, in which case
/// it looks in the namespace the user chose.
#[derive(Default)]
struct Scope {
    namespaced: bool,
    cluster_forbidden: bool,
}

impl Scope {
    fn wanted(&self, chosen: Option<&str>) -> Option<String> {
        let fall_back = self.namespaced && self.cluster_forbidden;
        chosen.filter(|_| fall_back).map(str::to_owned)
    }

    /// Learns from a list across all namespaces whether the API allows it.
    fn learn(&mut self, error: Option<&WatchError>, synced: bool) {
        match error {
            Some(error) if error.is_forbidden() => self.cluster_forbidden = true,
            None if synced => self.cluster_forbidden = false,
            _ => {}
        }
    }
}

enum Ended {
    /// The stream or every receiver has gone.
    Closed,
    /// The watch should look somewhere else.
    Moved,
}

/// Publishes at most once per interval, and not at all while the stream is quiet. Returns
/// when the watch should move to another namespace.
async fn publish<R: Row>(
    events: impl Stream<Item = watcher::Result<Event<R::Object>>>,
    snapshots: &watch::Sender<Arc<Snapshot<R>>>,
    looking_in: Option<String>,
    chosen: &mut watch::Receiver<Option<String>>,
    scope: &mut Scope,
) -> Ended {
    let mut events = pin!(events);
    let mut state = State::<R>::default();
    let namespace = looking_in.as_deref();
    loop {
        tokio::select! {
            event = events.next() => {
                let Some(event) = event else {
                    return Ended::Closed;
                };
                state.record(event, namespace);
                let mut deadline = pin!(tokio::time::sleep(PUBLISH_INTERVAL));
                let mut ended = false;
                while !ended {
                    tokio::select! {
                        () = &mut deadline => break,
                        event = events.next() => match event {
                            Some(event) => state.record(event, namespace),
                            None => ended = true,
                        },
                    }
                }
                if looking_in.is_none() {
                    scope.learn(state.error.as_ref(), state.synced);
                }
                if scope.wanted(chosen.borrow().as_deref()) != looking_in {
                    return Ended::Moved;
                }
                if snapshots.send(Arc::new(state.snapshot())).is_err() || ended {
                    return Ended::Closed;
                }
            }
            changed = chosen.changed() => {
                if changed.is_err() {
                    return Ended::Closed;
                }
                if scope.wanted(chosen.borrow_and_update().as_deref()) != looking_in {
                    return Ended::Moved;
                }
            }
        }
    }
}

async fn run<R: Row>(
    client: Client,
    mut chosen: watch::Receiver<Option<String>>,
    snapshots: watch::Sender<Arc<Snapshot<R>>>,
) {
    let mut scope = Scope {
        namespaced: R::NAMESPACED,
        ..Scope::default()
    };
    let mut first = true;
    loop {
        let looking_in = scope.wanted(chosen.borrow_and_update().as_deref());
        if !first && snapshots.send(Arc::default()).is_err() {
            return;
        }
        first = false;
        let api = R::api(client.clone(), looking_in.as_deref());
        let events = watcher::watcher(api, watcher::Config::default()).default_backoff();
        match publish(events, &snapshots, looking_in, &mut chosen, &mut scope).await {
            Ended::Closed => return,
            Ended::Moved => {}
        }
    }
}

/// The newest snapshots from a running watch, for anything that follows objects as they
/// change.
pub struct Feed<R>(pub(crate) watch::Receiver<Arc<Snapshot<R>>>);

impl<R> Clone for Feed<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<R> Feed<R> {
    /// Waits for the newest snapshot, or returns `None` if the watch has ended.
    pub async fn next(&mut self) -> Option<Arc<Snapshot<R>>> {
        self.0.changed().await.ok()?;
        Some(self.0.borrow_and_update().clone())
    }
}

/// A running watch of one kind of object. Dropping it stops the watch.
pub struct ResourceWatch<R> {
    snapshots: watch::Receiver<Arc<Snapshot<R>>>,
    chosen: watch::Sender<Option<String>>,
    task: JoinHandle<()>,
}

impl<R> ResourceWatch<R> {
    /// Waits for the newest snapshot, or returns `None` if the watch has ended.
    pub async fn next(&mut self) -> Option<Arc<Snapshot<R>>> {
        self.snapshots.changed().await.ok()?;
        Some(self.snapshots.borrow_and_update().clone())
    }

    /// Shares this watch, so a view or a log tail can follow it without a watch of its own.
    pub fn feed(&self) -> Feed<R> {
        Feed(self.snapshots.clone())
    }

    /// Tells the watch which namespace the user chose. It only looks there if it may not
    /// list across all namespaces.
    pub fn set_namespace(&self, namespace: Option<String>) {
        self.chosen.send_if_modified(|chosen| {
            let changed = *chosen != namespace;
            *chosen = namespace;
            changed
        });
    }
}

impl<R> Drop for ResourceWatch<R> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts a watch of `R` on the current Tokio runtime.
pub(crate) fn start<R: Row>(client: Client, namespace: Option<String>) -> ResourceWatch<R> {
    let (sender, snapshots) = watch::channel(Arc::default());
    let (chosen, chosen_receiver) = watch::channel(namespace);
    let task = tokio::spawn(run::<R>(client, chosen_receiver, sender));
    ResourceWatch {
        snapshots,
        chosen,
        task,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use futures::stream;
    use k8s_openapi::api::core::v1::Pod;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::fake_api::{self, Handler, Reply, Request};
    use crate::pods::PodRow;

    type PodState = State<PodRow>;

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

    fn names(snapshot: &Snapshot<PodRow>) -> Vec<&str> {
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

    /// Publishes `events` with no namespace chosen, as a view of all namespaces does.
    async fn publish_all(
        events: impl Stream<Item = watcher::Result<Event<Pod>>>,
        sender: watch::Sender<Arc<Snapshot<PodRow>>>,
    ) {
        let (_chosen, mut receiver) = watch::channel(None);
        publish(events, &sender, None, &mut receiver, &mut Scope::default()).await;
    }

    #[test]
    fn find_locates_a_row_by_namespace_and_name() {
        let snapshot = listed(vec![pod("b", "api"), pod("a", "web"), pod("a", "api")]).snapshot();
        let key = ("a".to_owned(), "web".to_owned());
        assert_eq!(snapshot.find(&key).unwrap().name, "web");
        assert!(snapshot.find(&("c".to_owned(), "api".to_owned())).is_none());
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
    fn a_relist_drops_objects_deleted_during_a_disconnect() {
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

        state.record(Err(watcher::Error::NoResourceVersion), None);
        let failed = state.snapshot();
        state.record(Ok(Event::Apply(pod("a", "web"))), None);
        let recovered = state.snapshot();

        assert_eq!(names(&failed), ["api"]);
        assert!(matches!(failed.error, Some(WatchError::Failed { .. })));
        assert_eq!(recovered.error, None);
    }

    fn api_error(code: u16, message: &str) -> watcher::Error {
        let status = kube::core::Status::failure(message, "").with_code(code);
        watcher::Error::InitialListFailed(kube::Error::Api(status.boxed()))
    }

    #[test]
    fn errors_tell_a_refusal_from_a_missing_kind_and_a_failure() {
        let forbidden = WatchError::from_watcher(&api_error(403, "no list"), Some("shop"));
        let missing = WatchError::from_watcher(&api_error(404, "not found"), None);
        let failed = WatchError::from_watcher(&api_error(500, "broken"), None);

        assert_eq!(
            forbidden,
            WatchError::Forbidden {
                namespace: Some("shop".into()),
                message: "no list".into(),
            }
        );
        assert_eq!(
            missing,
            WatchError::NotServed {
                message: "not found".into()
            }
        );
        assert!(matches!(failed, WatchError::Failed { .. }));
        assert_eq!(forbidden.to_string(), "no list");
    }

    #[test]
    fn a_refused_cluster_list_falls_back_to_the_chosen_namespace() {
        let mut scope = Scope {
            namespaced: true,
            ..Scope::default()
        };
        assert_eq!(scope.wanted(Some("shop")), None);

        scope.learn(
            Some(&WatchError::Forbidden {
                namespace: None,
                message: String::new(),
            }),
            false,
        );

        assert_eq!(scope.wanted(Some("shop")).as_deref(), Some("shop"));
        assert_eq!(scope.wanted(None), None);
        scope.learn(None, true);
        assert_eq!(scope.wanted(Some("shop")), None);
    }

    #[test]
    fn a_cluster_scoped_kind_never_falls_back() {
        let mut scope = Scope::default();
        scope.learn(
            Some(&WatchError::Forbidden {
                namespace: None,
                message: String::new(),
            }),
            false,
        );
        assert_eq!(scope.wanted(Some("shop")), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_events_publishes_one_snapshot() {
        let (events, received) = mpsc::unbounded_channel();
        let (sender, mut snapshots) = watch::channel(Arc::default());
        let task = tokio::spawn(publish_all(channel_stream(received), sender));

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

        publish_all(events, sender).await;

        assert!(snapshots.borrow_and_update().synced);
    }

    #[tokio::test]
    async fn dropping_a_watch_stops_its_task() {
        let (sender, snapshots) = watch::channel(Arc::<Snapshot<PodRow>>::default());
        let (chosen, _) = watch::channel(None);
        let task = tokio::spawn(async move {
            sender.closed().await;
        });
        let abort = task.abort_handle();

        drop(ResourceWatch {
            snapshots,
            chosen,
            task,
        });
        tokio::task::yield_now().await;

        assert!(abort.is_finished());
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"}}]}"#;

    /// Answers lists from `lists` by path, holds every watch open, and records each path.
    fn serve_lists(lists: Vec<(&'static str, Reply)>, seen: Arc<Mutex<Vec<String>>>) -> Handler {
        let lists = Arc::new(Mutex::new(lists));
        Arc::new(move |request: &Request| {
            if request
                .query
                .get("watch")
                .is_some_and(|watch| watch == "true")
            {
                return Some(Reply::Hang);
            }
            if request.path == "/version" {
                return None;
            }
            seen.lock().unwrap().push(request.path.clone());
            let lists = lists.lock().unwrap();
            let (_, reply) = lists.iter().find(|(path, _)| *path == request.path)?;
            Some(reply.clone())
        })
    }

    async fn next_settled(watch: &mut ResourceWatch<PodRow>) -> Arc<Snapshot<PodRow>> {
        loop {
            let snapshot = tokio::time::timeout(Duration::from_secs(10), watch.next())
                .await
                .expect("the watch did not publish")
                .expect("the watch ended");
            if snapshot.synced || snapshot.error.is_some() {
                return snapshot;
            }
        }
    }

    #[tokio::test]
    async fn a_refused_list_says_so_and_names_where() {
        let seen = Arc::default();
        let handler = serve_lists(
            vec![("/api/v1/pods", Reply::Forbidden("pods is forbidden"))],
            Arc::clone(&seen),
        );
        let connection = fake_api::connect_with(handler).await;
        let mut watch = start::<PodRow>(connection.client, None);

        let snapshot = next_settled(&mut watch).await;

        assert_eq!(
            snapshot.error,
            Some(WatchError::Forbidden {
                namespace: None,
                message: "pods is forbidden".into(),
            })
        );
    }

    #[tokio::test]
    async fn a_refused_cluster_list_watches_the_chosen_namespace_instead() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler = serve_lists(
            vec![
                ("/api/v1/pods", Reply::Forbidden("pods is forbidden")),
                ("/api/v1/namespaces/shop/pods", Reply::Json(POD_LIST.into())),
            ],
            Arc::clone(&seen),
        );
        let connection = fake_api::connect_with(handler).await;
        let mut watch = start::<PodRow>(connection.client, Some("shop".into()));

        let snapshot = next_settled(&mut watch).await;

        assert_eq!(snapshot.error, None);
        assert_eq!(names(&snapshot), ["api-1"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], "/api/v1/pods");
        assert!(seen.contains(&"/api/v1/namespaces/shop/pods".to_owned()));
    }

    #[tokio::test]
    async fn an_allowed_cluster_list_ignores_the_chosen_namespace() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler = serve_lists(
            vec![("/api/v1/pods", Reply::Json(POD_LIST.into()))],
            Arc::clone(&seen),
        );
        let connection = fake_api::connect_with(handler).await;
        let mut watch = start::<PodRow>(connection.client, None);
        next_settled(&mut watch).await;

        watch.set_namespace(Some("web".into()));
        tokio::time::sleep(PUBLISH_INTERVAL * 3).await;

        assert_eq!(*seen.lock().unwrap(), ["/api/v1/pods"]);
    }
}
