use std::fmt::Debug;

use k8s_openapi::api::core::v1::{ContainerState, ContainerStatus, Event, Pod};
use kube::api::ListParams;
use kube::{Api, Client, Resource};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cluster::ClusterError;

const MAX_EVENTS: usize = 100;

/// What the details panel shows of one kind of object beyond its table row, read when the
/// panel opens and again while it stays open.
pub trait ObjectDetails: Clone + Debug + PartialEq + Send + Sized + 'static {
    type Object: Resource<DynamicType = ()>
        + Clone
        + Debug
        + DeserializeOwned
        + Serialize
        + Send
        + 'static;

    /// Reads objects in `namespace`.
    fn api(client: Client, namespace: &str) -> Api<Self::Object>;

    /// Builds the details from an object and its events, which a role may not be allowed
    /// to list.
    fn read(object: Self::Object, events: Result<Vec<ObjectEvent>, String>)
    -> Result<Self, String>;

    /// The object as read-only YAML, without `managedFields`.
    fn yaml(&self) -> &str;

    fn events(&self) -> &Result<Vec<ObjectEvent>, String>;
}

/// What the details panel shows for one pod, read once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodDetails {
    pub owner: Option<String>,
    pub pod_ip: Option<String>,
    pub containers: Vec<ContainerDetails>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    /// Kept apart from the pod, because a role can read pods without reading events.
    pub events: Result<Vec<ObjectEvent>, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerDetails {
    pub name: String,
    pub image: String,
    pub state: String,
    pub ready: bool,
    pub restarts: i32,
    pub last_exit: Option<String>,
}

/// An event about one object, such as a pod's `BackOff`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectEvent {
    pub warning: bool,
    pub reason: String,
    pub message: String,
    pub count: i32,
    pub last_seen_unix: Option<i64>,
}

fn describe_state(state: &ContainerState) -> String {
    let reason = state
        .waiting
        .as_ref()
        .and_then(|waiting| waiting.reason.clone())
        .or_else(|| state.terminated.as_ref()?.reason.clone());
    match (reason, &state.running, &state.waiting) {
        (Some(reason), _, _) => reason,
        (None, Some(_), _) => "Running".to_owned(),
        (None, None, Some(_)) => "Waiting".to_owned(),
        (None, None, None) => "Terminated".to_owned(),
    }
}

fn describe_exit(status: &ContainerStatus) -> Option<String> {
    let terminated = status.last_state.as_ref()?.terminated.as_ref()?;
    let reason = terminated.reason.as_deref().unwrap_or("Terminated");
    Some(format!("{reason}, exit code {}", terminated.exit_code))
}

fn container_details(pod: &Pod) -> Vec<ContainerDetails> {
    let status = pod.status.as_ref();
    let statuses = status
        .and_then(|status| status.container_statuses.as_deref())
        .unwrap_or_default();
    pod.spec
        .iter()
        .flat_map(|spec| &spec.containers)
        .map(|container| {
            let status = statuses.iter().find(|status| status.name == container.name);
            ContainerDetails {
                name: container.name.clone(),
                image: container.image.clone().unwrap_or_default(),
                state: status
                    .and_then(|status| status.state.as_ref())
                    .map_or_else(|| "Unknown".to_owned(), describe_state),
                ready: status.is_some_and(|status| status.ready),
                restarts: status.map_or(0, |status| status.restart_count),
                last_exit: status.and_then(describe_exit),
            }
        })
        .collect()
}

fn last_seen_unix(event: &Event) -> Option<i64> {
    let seen = event.last_timestamp.as_ref().map(|time| time.0);
    let recorded = event.event_time.as_ref().map(|time| time.0);
    let first = event.first_timestamp.as_ref().map(|time| time.0);
    seen.or(recorded).or(first).map(|time| time.as_second())
}

fn newest_events(events: Vec<Event>) -> Vec<ObjectEvent> {
    let mut events: Vec<_> = events
        .iter()
        .map(|event| ObjectEvent {
            warning: event.type_.as_deref() == Some("Warning"),
            reason: event.reason.clone().unwrap_or_default(),
            message: event.message.clone().unwrap_or_default(),
            count: event.count.unwrap_or(1),
            last_seen_unix: last_seen_unix(event),
        })
        .collect();
    events.sort_by_key(|event| std::cmp::Reverse(event.last_seen_unix));
    events.truncate(MAX_EVENTS);
    events
}

/// An object as YAML. Field ownership is bookkeeping for the API server, and it is longer
/// than most objects, so it is left out.
pub(crate) fn to_yaml<K: Resource + Serialize>(object: &mut K) -> Result<String, String> {
    object.meta_mut().managed_fields = None;
    serde_saphyr::to_string(object).map_err(|error| error.to_string())
}

impl PodDetails {
    fn new(mut pod: Pod, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut pod)?;
        Ok(Self {
            owner: pod
                .metadata
                .owner_references
                .iter()
                .flatten()
                .find(|owner| owner.controller == Some(true))
                .map(|owner| format!("{}/{}", owner.kind, owner.name)),
            pod_ip: pod.status.as_ref().and_then(|status| status.pod_ip.clone()),
            containers: container_details(&pod),
            labels: pod
                .metadata
                .labels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            yaml,
            events,
        })
    }
}

impl ObjectDetails for PodDetails {
    type Object = Pod;

    fn api(client: Client, namespace: &str) -> Api<Pod> {
        Api::namespaced(client, namespace)
    }

    fn read(pod: Pod, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        Self::new(pod, events)
    }

    fn yaml(&self) -> &str {
        &self.yaml
    }

    fn events(&self) -> &Result<Vec<ObjectEvent>, String> {
        &self.events
    }
}

/// Reads one object and its newest events.
pub(crate) async fn load<D: ObjectDetails>(
    client: Client,
    namespace: String,
    name: String,
) -> Result<D, ClusterError> {
    let kind = D::Object::kind(&());
    let failed = |error: String| ClusterError::ReadObject {
        object: format!("{} {namespace}/{name}", kind.to_lowercase()),
        error,
    };
    let object = D::api(client.clone(), &namespace)
        .get(&name)
        .await
        .map_err(|error| failed(error.to_string()))?;
    let selector = format!("involvedObject.kind={kind},involvedObject.name={name}");
    let events = Api::<Event>::namespaced(client, &namespace)
        .list(&ListParams::default().fields(&selector))
        .await
        .map(|events| newest_events(events.items))
        .map_err(|error| error.to_string());
    D::read(object, events).map_err(failed)
}

#[cfg(test)]
mod tests {
    use serde_json::{from_value, json};

    use super::{ContainerDetails, ObjectEvent, PodDetails, newest_events};
    use k8s_openapi::api::core::v1::{Event, Pod};

    fn pod() -> Pod {
        from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "api-1",
                "labels": {"app": "api", "tier": "web"},
                "managedFields": [{"manager": "kubectl"}],
                "ownerReferences": [
                    {"apiVersion": "v1", "kind": "Node", "name": "other", "uid": "1"},
                    {"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "api-7f9c",
                     "uid": "2", "controller": true}
                ]
            },
            "spec": {"containers": [
                {"name": "app", "image": "api:2.31.0"},
                {"name": "proxy", "image": "envoy:1.30"}
            ]},
            "status": {
                "podIP": "10.4.0.7",
                "containerStatuses": [{
                    "name": "app", "image": "api:2.31.0", "imageID": "x", "ready": false,
                    "restartCount": 7,
                    "state": {"waiting": {"reason": "CrashLoopBackOff"}},
                    "lastState": {"terminated": {"exitCode": 1, "reason": "Error"}}
                }]
            }
        }))
        .unwrap()
    }

    fn event(kind: &str, reason: &str, last_seen: &str) -> Event {
        from_value(json!({
            "metadata": {"name": reason, "namespace": "shop"},
            "involvedObject": {"kind": "Pod", "name": "api-1"},
            "type": kind, "reason": reason, "message": "details", "count": 3,
            "lastTimestamp": last_seen
        }))
        .unwrap()
    }

    #[test]
    fn summarizes_the_owner_address_and_labels() {
        let details = PodDetails::new(pod(), Ok(vec![])).unwrap();
        assert_eq!(details.owner.as_deref(), Some("ReplicaSet/api-7f9c"));
        assert_eq!(details.pod_ip.as_deref(), Some("10.4.0.7"));
        assert_eq!(details.labels[0], ("app".to_owned(), "api".to_owned()));
    }

    #[test]
    fn describes_each_container_even_without_a_status() {
        let details = PodDetails::new(pod(), Ok(vec![])).unwrap();
        assert_eq!(
            details.containers,
            vec![
                ContainerDetails {
                    name: "app".into(),
                    image: "api:2.31.0".into(),
                    state: "CrashLoopBackOff".into(),
                    ready: false,
                    restarts: 7,
                    last_exit: Some("Error, exit code 1".into()),
                },
                ContainerDetails {
                    name: "proxy".into(),
                    image: "envoy:1.30".into(),
                    state: "Unknown".into(),
                    ready: false,
                    restarts: 0,
                    last_exit: None,
                },
            ]
        );
    }

    #[test]
    fn yaml_keeps_the_pod_and_drops_field_ownership() {
        let details = PodDetails::new(pod(), Ok(vec![])).unwrap();
        assert!(details.yaml.contains("name: api-1"), "{}", details.yaml);
        assert!(details.yaml.contains("kind: Pod"), "{}", details.yaml);
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    #[test]
    fn events_are_newest_first() {
        let events = vec![
            event("Normal", "Pulled", "2026-11-07T10:00:00Z"),
            event("Warning", "BackOff", "2026-11-07T10:05:00Z"),
        ];
        let details = PodDetails::new(pod(), Ok(newest_events(events))).unwrap();
        assert_eq!(
            details.events.unwrap(),
            vec![
                ObjectEvent {
                    warning: true,
                    reason: "BackOff".into(),
                    message: "details".into(),
                    count: 3,
                    last_seen_unix: Some(1_794_045_900),
                },
                ObjectEvent {
                    warning: false,
                    reason: "Pulled".into(),
                    message: "details".into(),
                    count: 3,
                    last_seen_unix: Some(1_794_045_600),
                },
            ]
        );
    }

    #[test]
    fn a_failed_event_list_keeps_the_rest_of_the_details() {
        let details = PodDetails::new(pod(), Err("forbidden".into())).unwrap();
        assert_eq!(details.events, Err("forbidden".to_owned()));
        assert_eq!(details.containers.len(), 2);
    }
}
