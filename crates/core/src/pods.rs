use std::collections::BTreeMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client};

use crate::logs::{LogTarget, MAX_TAIL_PODS};
use crate::watch::{Feed, ObjectKey, ResourceWatch, Row, Snapshot, namespaced_api};

pub type PodKey = ObjectKey;
/// Pods at one moment, sorted by namespace and name.
pub type PodSnapshot = Snapshot<PodRow>;
/// The newest pods from a running watch, for anything that follows pods as they change.
pub type PodFeed = Feed<PodRow>;
/// A running pod watch. Dropping it stops the watch.
pub type PodWatch = ResourceWatch<PodRow>;

/// The object that controls a pod, such as its ReplicaSet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub kind: String,
    pub name: String,
}

/// The fields of a pod that the app uses, so full objects need not stay in memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodRow {
    pub namespace: String,
    pub name: String,
    /// Tells a pod apart from an earlier one with the same name, as a StatefulSet makes.
    pub uid: String,
    pub status: String,
    pub ready: String,
    pub restarts: i32,
    pub node: String,
    pub created_unix: Option<i64>,
    pub containers: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub owner: Option<Owner>,
}

impl Row for PodRow {
    type Object = Pod;

    fn from_object(pod: &Pod) -> Self {
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
            uid: pod.metadata.uid.clone().unwrap_or_default(),
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
            labels: pod.metadata.labels.clone().unwrap_or_default(),
            owner: pod
                .metadata
                .owner_references
                .iter()
                .flatten()
                .find(|owner| owner.controller == Some(true))
                .map(|owner| Owner {
                    kind: owner.kind.clone(),
                    name: owner.name.clone(),
                }),
        }
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<Pod> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl From<&Pod> for PodRow {
    fn from(pod: &Pod) -> Self {
        Self::from_object(pod)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(namespace: &str, name: &str) -> Pod {
        serde_json::from_value(json!({
            "metadata": { "namespace": namespace, "name": name },
        }))
        .unwrap()
    }

    fn listed(pods: Vec<Pod>) -> PodSnapshot {
        let mut rows: Vec<_> = pods.iter().map(|pod| Arc::new(PodRow::from(pod))).collect();
        rows.sort_by_key(|row| row.key());
        PodSnapshot {
            rows,
            synced: true,
            error: None,
        }
    }

    #[test]
    fn row_summarizes_a_running_pod() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "api-1",
                "uid": "u-1",
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "labels": { "app": "api" },
                "ownerReferences": [
                    { "apiVersion": "v1", "kind": "Node", "name": "node-1", "uid": "n" },
                    { "apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "api-7f9c", "uid": "r", "controller": true },
                ],
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
                uid: "u-1".into(),
                status: "Running".into(),
                ready: "1/2".into(),
                restarts: 5,
                node: "node-1".into(),
                created_unix: Some(1_767_225_600),
                containers: vec!["app".into(), "proxy".into()],
                labels: BTreeMap::from([("app".into(), "api".into())]),
                owner: Some(Owner {
                    kind: "ReplicaSet".into(),
                    name: "api-7f9c".into(),
                }),
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
        ]);

        let targets = log_targets(snapshot.rows.iter().rev());

        let names: Vec<_> = targets
            .iter()
            .map(|t| (t.pod.as_str(), t.container.as_str()))
            .collect();
        assert_eq!(names, [("web", "app"), ("api", "app"), ("api", "proxy")]);
    }

    #[test]
    fn log_targets_stop_at_the_pod_limit() {
        let pods = (0..MAX_TAIL_PODS + 5)
            .map(|index| pod_with_containers(&format!("pod-{index:03}"), &["app"]))
            .collect();

        let targets = log_targets(&listed(pods).rows);

        assert_eq!(targets.len(), MAX_TAIL_PODS);
    }
}
