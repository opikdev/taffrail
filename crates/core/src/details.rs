use k8s_openapi::api::core::v1::{ContainerState, ContainerStatus, Event, Pod};
use kube::api::ListParams;
use kube::{Api, Client};

use crate::cluster::ClusterError;

const MAX_EVENTS: usize = 100;

/// What the details panel shows for one pod, read once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodDetails {
    pub owner: Option<String>,
    pub pod_ip: Option<String>,
    pub containers: Vec<ContainerDetails>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    /// Kept apart from the pod, because a role can read pods without reading events.
    pub events: Result<Vec<PodEvent>, String>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodEvent {
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

fn newest_events(events: Vec<Event>) -> Vec<PodEvent> {
    let mut events: Vec<_> = events
        .iter()
        .map(|event| PodEvent {
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

fn to_yaml(pod: &mut Pod) -> Result<String, serde_saphyr::ser_error::Error> {
    // Field ownership is bookkeeping for the API server, and it is longer than the pod.
    pod.metadata.managed_fields = None;
    serde_saphyr::to_string(pod)
}

impl PodDetails {
    fn new(mut pod: Pod, events: Result<Vec<Event>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut pod).map_err(|error| error.to_string())?;
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
            events: events.map(newest_events),
        })
    }
}

pub(crate) async fn load(
    client: Client,
    namespace: String,
    name: String,
) -> Result<PodDetails, ClusterError> {
    let failed = |error: String| ClusterError::PodDetails {
        pod: format!("{namespace}/{name}"),
        error,
    };
    let pod = Api::<Pod>::namespaced(client.clone(), &namespace)
        .get(&name)
        .await
        .map_err(|error| failed(error.to_string()))?;
    let selector = format!("involvedObject.kind=Pod,involvedObject.name={name}");
    let events = Api::<Event>::namespaced(client, &namespace)
        .list(&ListParams::default().fields(&selector))
        .await
        .map(|events| events.items)
        .map_err(|error| error.to_string());
    PodDetails::new(pod, events).map_err(failed)
}

#[cfg(test)]
mod tests {
    use serde_json::{from_value, json};

    use super::{ContainerDetails, PodDetails, PodEvent};
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
        let details = PodDetails::new(pod(), Ok(events)).unwrap();
        assert_eq!(
            details.events.unwrap(),
            vec![
                PodEvent {
                    warning: true,
                    reason: "BackOff".into(),
                    message: "details".into(),
                    count: 3,
                    last_seen_unix: Some(1_794_045_900),
                },
                PodEvent {
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
