//! Events, read from `events.k8s.io/v1` where the cluster serves it, and from the core
//! `v1` API where it does not. Both give the same rows and details.

use std::ops::Deref;

use k8s_openapi::api::core::v1::{Event as CoreEvent, ObjectReference};
use k8s_openapi::api::events::v1::Event;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::{Api, Client};

use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::watch::{Row, namespaced_api};

/// The longest message a row keeps. The details panel reads the whole message.
const ROW_MESSAGE_CHARS: usize = 240;

/// The fields both event APIs share, read from either.
struct Fields {
    warning: bool,
    reason: String,
    regarding: ObjectReference,
    note: String,
    count: i32,
    first_seen_unix: Option<i64>,
    last_seen_unix: Option<i64>,
    /// What recorded it, such as `kubelet, node-07`.
    source: String,
    action: String,
    related: Option<ObjectReference>,
}

fn created(metadata: &ObjectMeta) -> Option<i64> {
    metadata
        .creation_timestamp
        .as_ref()
        .map(|time| time.0.as_second())
}

fn joined(parts: [Option<&String>; 2]) -> String {
    let parts: Vec<&str> = parts
        .into_iter()
        .flatten()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .collect();
    parts.join(", ")
}

impl From<&Event> for Fields {
    fn from(event: &Event) -> Self {
        let series = event.series.as_ref();
        let last_seen = series
            .map(|series| series.last_observed_time.0.as_second())
            .or(event
                .deprecated_last_timestamp
                .as_ref()
                .map(|time| time.0.as_second()))
            .or(event.event_time.as_ref().map(|time| time.0.as_second()))
            .or(created(&event.metadata));
        let first_seen = event
            .deprecated_first_timestamp
            .as_ref()
            .map(|time| time.0.as_second())
            .or(event.event_time.as_ref().map(|time| time.0.as_second()))
            .or(created(&event.metadata));
        let legacy = event.deprecated_source.as_ref();
        let source = match &event.reporting_controller {
            Some(controller) if !controller.is_empty() => {
                joined([Some(controller), event.reporting_instance.as_ref()])
            }
            _ => joined([
                legacy.and_then(|source| source.component.as_ref()),
                legacy.and_then(|source| source.host.as_ref()),
            ]),
        };
        Self {
            warning: event.type_.as_deref() == Some("Warning"),
            reason: event.reason.clone().unwrap_or_default(),
            regarding: event.regarding.clone().unwrap_or_default(),
            note: event.note.clone().unwrap_or_default(),
            count: series
                .map(|series| series.count)
                .or(event.deprecated_count)
                .unwrap_or(1),
            first_seen_unix: first_seen,
            last_seen_unix: last_seen,
            source,
            action: event.action.clone().unwrap_or_default(),
            related: event.related.clone(),
        }
    }
}

impl From<&CoreEvent> for Fields {
    fn from(event: &CoreEvent) -> Self {
        let series = event.series.as_ref();
        let last_seen = series
            .and_then(|series| series.last_observed_time.as_ref())
            .map(|time| time.0.as_second())
            .or(event.last_timestamp.as_ref().map(|time| time.0.as_second()))
            .or(event.event_time.as_ref().map(|time| time.0.as_second()))
            .or(event
                .first_timestamp
                .as_ref()
                .map(|time| time.0.as_second()))
            .or(created(&event.metadata));
        let first_seen = event
            .first_timestamp
            .as_ref()
            .map(|time| time.0.as_second())
            .or(event.event_time.as_ref().map(|time| time.0.as_second()))
            .or(created(&event.metadata));
        let legacy = event.source.as_ref();
        let source = match &event.reporting_component {
            Some(component) if !component.is_empty() => {
                joined([Some(component), event.reporting_instance.as_ref()])
            }
            _ => joined([
                legacy.and_then(|source| source.component.as_ref()),
                legacy.and_then(|source| source.host.as_ref()),
            ]),
        };
        Self {
            warning: event.type_.as_deref() == Some("Warning"),
            reason: event.reason.clone().unwrap_or_default(),
            regarding: event.involved_object.clone(),
            note: event.message.clone().unwrap_or_default(),
            count: series
                .and_then(|series| series.count)
                .or(event.count)
                .unwrap_or(1),
            first_seen_unix: first_seen,
            last_seen_unix: last_seen,
            source,
            action: event.action.clone().unwrap_or_default(),
            related: event.related.clone(),
        }
    }
}

/// The API group of an object reference: `apps` for `apps/v1`, and empty for the core API.
fn group(reference: &ObjectReference) -> String {
    reference
        .api_version
        .as_deref()
        .and_then(|version| version.rsplit_once('/'))
        .map(|(group, _)| group.to_owned())
        .unwrap_or_default()
}

/// Such as `pod/checkout-api-7f9c`.
fn describe(reference: &ObjectReference) -> String {
    let kind = reference.kind.as_deref().unwrap_or_default().to_lowercase();
    let name = reference.name.as_deref().unwrap_or_default();
    format!("{kind}/{name}")
}

/// The first line of a message, cut short for a row.
fn row_message(note: &str) -> String {
    let line = note.lines().next().unwrap_or_default().trim_end();
    match line.char_indices().nth(ROW_MESSAGE_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

/// The fields of an event that its table row and actions use. Clusters can hold tens of
/// thousands of events, so a row keeps only the first line of a long message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventRow {
    pub namespace: String,
    pub name: String,
    pub warning: bool,
    pub reason: String,
    /// The kind of object it is about, such as `Pod`.
    pub object_kind: String,
    pub object_name: String,
    /// Empty for a cluster-scoped object, such as a node.
    pub object_namespace: String,
    /// The object's API group: empty for the core API, or such as `apps`.
    pub object_group: String,
    pub message: String,
    pub count: i32,
    pub last_seen_unix: Option<i64>,
}

impl EventRow {
    /// `Warning` or `Normal`.
    pub fn kind(&self) -> &'static str {
        if self.warning { "Warning" } else { "Normal" }
    }

    /// Such as `pod/checkout-api-7f9c`.
    pub fn object(&self) -> String {
        format!("{}/{}", self.object_kind.to_lowercase(), self.object_name)
    }

    fn new(metadata: &ObjectMeta, fields: Fields) -> Self {
        let regarding = &fields.regarding;
        Self {
            namespace: metadata.namespace.clone().unwrap_or_default(),
            name: metadata.name.clone().unwrap_or_default(),
            warning: fields.warning,
            reason: fields.reason,
            object_kind: regarding.kind.clone().unwrap_or_default(),
            object_name: regarding.name.clone().unwrap_or_default(),
            object_namespace: regarding.namespace.clone().unwrap_or_default(),
            object_group: group(regarding),
            message: row_message(&fields.note),
            count: fields.count,
            last_seen_unix: fields.last_seen_unix,
        }
    }

    /// Whether `filter`, already in lowercase, appears in its namespace, reason, object or
    /// message, ignoring case.
    fn matches_text(&self, filter: &str) -> bool {
        let found = |text: &str| text.to_lowercase().contains(filter);
        filter.is_empty()
            || found(&self.namespace)
            || found(&self.reason)
            || found(&self.object())
            || found(&self.message)
    }
}

impl Row for EventRow {
    type Object = Event;

    fn from_object(event: &Event) -> Self {
        Self::new(&event.metadata, event.into())
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<Event> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn matches(&self, filter: &str) -> bool {
        self.matches_text(filter)
    }
}

/// An event row read from the core `v1` API, for a cluster that does not serve
/// `events.k8s.io/v1`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreEventRow(pub EventRow);

impl Deref for CoreEventRow {
    type Target = EventRow;

    fn deref(&self) -> &EventRow {
        &self.0
    }
}

impl Row for CoreEventRow {
    type Object = CoreEvent;

    fn from_object(event: &CoreEvent) -> Self {
        Self(EventRow::new(&event.metadata, event.into()))
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<CoreEvent> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.0.namespace
    }

    fn name(&self) -> &str {
        &self.0.name
    }

    fn matches(&self, filter: &str) -> bool {
        self.0.matches_text(filter)
    }
}

/// What the details panel shows for one event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventDetails {
    /// The API it was read from: `events.k8s.io/v1` or `v1`.
    pub api: &'static str,
    /// The whole message.
    pub message: String,
    pub first_seen_unix: Option<i64>,
    pub source: String,
    pub action: String,
    /// Another object it involves, such as `node/node-07`.
    pub related: Option<String>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

impl EventDetails {
    fn new(
        api: &'static str,
        fields: Fields,
        yaml: String,
        events: Result<Vec<ObjectEvent>, String>,
    ) -> Self {
        Self {
            api,
            message: fields.note,
            first_seen_unix: fields.first_seen_unix,
            source: fields.source,
            action: fields.action,
            related: fields.related.as_ref().map(describe),
            yaml,
            events,
        }
    }
}

impl AsRef<EventDetails> for EventDetails {
    fn as_ref(&self) -> &EventDetails {
        self
    }
}

impl ObjectDetails for EventDetails {
    type Object = Event;

    fn api(client: Client, namespace: &str) -> Api<Event> {
        Api::namespaced(client, namespace)
    }

    fn read(mut event: Event, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut event)?;
        Ok(Self::new("events.k8s.io/v1", (&event).into(), yaml, events))
    }

    fn yaml(&self) -> &str {
        &self.yaml
    }

    fn events(&self) -> &Result<Vec<ObjectEvent>, String> {
        &self.events
    }
}

/// An event's details read from the core `v1` API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreEventDetails(pub EventDetails);

impl Deref for CoreEventDetails {
    type Target = EventDetails;

    fn deref(&self) -> &EventDetails {
        &self.0
    }
}

impl AsRef<EventDetails> for CoreEventDetails {
    fn as_ref(&self) -> &EventDetails {
        &self.0
    }
}

impl ObjectDetails for CoreEventDetails {
    type Object = CoreEvent;

    fn api(client: Client, namespace: &str) -> Api<CoreEvent> {
        Api::namespaced(client, namespace)
    }

    fn read(
        mut event: CoreEvent,
        events: Result<Vec<ObjectEvent>, String>,
    ) -> Result<Self, String> {
        let yaml = to_yaml(&mut event)?;
        let details = EventDetails::new("v1", (&event).into(), yaml, events);
        Ok(Self(details))
    }

    fn yaml(&self) -> &str {
        &self.0.yaml
    }

    fn events(&self) -> &Result<Vec<ObjectEvent>, String> {
        &self.0.events
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::fake_api::{self, Reply, Request};
    use crate::pods::PodRow;
    use crate::watch::{ResourceWatch, Snapshot, WatchError, start};

    fn event(extra: Value) -> Event {
        let mut object = json!({
            "metadata": {
                "namespace": "shop",
                "name": "api-1.17a3",
                "creationTimestamp": "2026-11-07T10:00:00Z",
                "managedFields": [{ "manager": "kubelet" }],
            },
            "eventTime": "2026-11-07T10:00:00.000000Z",
            "type": "Warning",
            "reason": "BackOff",
            "regarding": { "apiVersion": "v1", "kind": "Pod", "namespace": "shop", "name": "api-1" },
            "note": "Back-off restarting failed container app\nsecond line",
            "reportingController": "kubelet",
            "reportingInstance": "node-07",
            "action": "Restarting",
        });
        for (key, value) in extra.as_object().unwrap() {
            object[key] = value.clone();
        }
        serde_json::from_value(object).unwrap()
    }

    fn core_event() -> CoreEvent {
        serde_json::from_value(json!({
            "metadata": { "namespace": "shop", "name": "api.1", "managedFields": [{ "manager": "x" }] },
            "involvedObject": { "apiVersion": "apps/v1", "kind": "Deployment", "namespace": "shop", "name": "api" },
            "type": "Normal",
            "reason": "ScalingReplicaSet",
            "message": "Scaled up replica set api-7f9c to 3",
            "count": 4,
            "firstTimestamp": "2026-11-07T09:00:00Z",
            "lastTimestamp": "2026-11-07T10:05:00Z",
            "source": { "component": "deployment-controller" },
        }))
        .unwrap()
    }

    #[test]
    fn a_row_reads_the_object_message_and_last_seen() {
        let row = EventRow::from_object(&event(json!({})));

        assert_eq!(
            (row.namespace.as_str(), row.name.as_str()),
            ("shop", "api-1.17a3")
        );
        assert!(row.warning);
        assert_eq!(row.kind(), "Warning");
        assert_eq!(row.reason, "BackOff");
        assert_eq!(row.object(), "pod/api-1");
        assert_eq!(row.object_group, "");
        assert_eq!(row.message, "Back-off restarting failed container app");
        assert_eq!(row.count, 1);
        assert_eq!(row.last_seen_unix, Some(1_794_045_600));
    }

    #[test]
    fn a_series_counts_and_dates_a_repeated_event() {
        let series =
            json!({ "series": { "count": 41, "lastObservedTime": "2026-11-07T10:46:00.000000Z" } });
        let row = EventRow::from_object(&event(series));
        assert_eq!(row.count, 41);
        assert_eq!(row.last_seen_unix, Some(1_794_048_360));

        let legacy =
            json!({ "deprecatedCount": 7, "deprecatedLastTimestamp": "2026-11-07T10:01:00Z" });
        let row = EventRow::from_object(&event(legacy));
        assert_eq!((row.count, row.last_seen_unix), (7, Some(1_794_045_660)));
    }

    #[test]
    fn a_long_message_is_cut_short_in_the_row_only() {
        let long = "x".repeat(1_000);
        let row = EventRow::from_object(&event(json!({ "note": long })));
        assert_eq!(row.message.chars().count(), ROW_MESSAGE_CHARS + 1);
        assert!(row.message.ends_with('…'));

        let details = EventDetails::read(event(json!({ "note": long })), Ok(vec![])).unwrap();
        assert_eq!(details.message.len(), 1_000);
    }

    #[test]
    fn the_filter_searches_reason_object_and_message() {
        let row = EventRow::from_object(&event(json!({})));
        assert!(row.matches("backoff"));
        assert!(row.matches("pod/api-1"));
        assert!(row.matches("restarting failed"));
        assert!(row.matches("shop"));
        assert!(row.matches(""));
        assert!(!row.matches("17a3"));
    }

    #[test]
    fn the_core_api_gives_the_same_row() {
        let row = CoreEventRow::from_object(&core_event());
        assert!(!row.warning);
        assert_eq!(row.object(), "deployment/api");
        assert_eq!(row.object_group, "apps");
        assert_eq!(row.count, 4);
        assert_eq!(row.last_seen_unix, Some(1_794_045_900));
        assert_eq!(Row::name(&row), "api.1");

        let details = CoreEventDetails::read(core_event(), Ok(vec![])).unwrap();
        assert_eq!(details.api, "v1");
        assert_eq!(details.source, "deployment-controller");
        assert_eq!(details.first_seen_unix, Some(1_794_042_000));
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    #[test]
    fn details_name_the_api_source_and_related_object() {
        let related = json!({ "related": { "kind": "Node", "name": "node-07" } });
        let details = EventDetails::read(event(related), Ok(vec![])).unwrap();
        assert_eq!(details.api, "events.k8s.io/v1");
        assert_eq!(details.source, "kubelet, node-07");
        assert_eq!(details.action, "Restarting");
        assert_eq!(details.related.as_deref(), Some("node/node-07"));
        assert!(details.message.contains("second line"));
        assert!(details.yaml.contains("reason: BackOff"), "{}", details.yaml);
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"}}]}"#;
    const EVENT_LIST: &str = r#"{"kind":"EventList","apiVersion":"events.k8s.io/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1.1","resourceVersion":"1"},"eventTime":null,"type":"Warning","reason":"BackOff","regarding":{"kind":"Pod","name":"api-1"},"note":"Back-off"}]}"#;
    const CORE_EVENT_LIST: &str = r#"{"kind":"EventList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1.2","resourceVersion":"1"},"involvedObject":{"kind":"Pod","name":"api-1"},"type":"Normal","reason":"Pulled","message":"Pulled"}]}"#;

    async fn settle<R: Row>(watch: &mut ResourceWatch<R>) -> Arc<Snapshot<R>> {
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

    fn serve(events: Reply) -> fake_api::Handler {
        let events = Mutex::new(events);
        Arc::new(move |request: &Request| {
            if request
                .query
                .get("watch")
                .is_some_and(|watch| watch == "true")
            {
                return Some(Reply::Hang);
            }
            match request.path.as_str() {
                "/api/v1/pods" => Some(Reply::Json(POD_LIST.into())),
                "/apis/events.k8s.io/v1/events" => Some(events.lock().unwrap().clone()),
                "/api/v1/events" => Some(Reply::Json(CORE_EVENT_LIST.into())),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn events_are_watched_from_events_k8s_io() {
        let connection = fake_api::connect_with(serve(Reply::Json(EVENT_LIST.into()))).await;
        let mut events = start::<EventRow>(connection.client, None);

        let snapshot = settle(&mut events).await;

        assert_eq!(snapshot.rows[0].reason, "BackOff");
    }

    #[tokio::test]
    async fn a_cluster_without_events_k8s_io_says_so_and_the_core_api_still_lists() {
        let connection = fake_api::connect_with(serve(Reply::Status(404))).await;
        let mut events = start::<EventRow>(connection.client.clone(), None);
        let mut core = start::<CoreEventRow>(connection.client, None);

        let missing = settle(&mut events).await;
        let listed = settle(&mut core).await;

        assert!(
            matches!(missing.error, Some(WatchError::NotServed { .. })),
            "{:?}",
            missing.error
        );
        assert_eq!(listed.rows[0].reason, "Pulled");
    }

    #[tokio::test]
    async fn a_refused_event_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "events.events.k8s.io is forbidden: User \\\"jane\\\" cannot list resource \\\"events\\\" in API group \\\"events.k8s.io\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut events = start::<EventRow>(connection.client, None);

        let refused = settle(&mut events).await;
        let listed = settle(&mut pods).await;

        let Some(WatchError::Forbidden { namespace, message }) = &refused.error else {
            panic!("expected a refusal, got {:?}", refused.error);
        };
        assert_eq!(*namespace, None);
        assert!(message.contains("cannot list resource"), "{message}");
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "api-1");
    }
}
