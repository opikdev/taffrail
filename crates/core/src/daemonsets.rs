use k8s_openapi::api::apps::v1::DaemonSet;
use k8s_openapi::api::core::v1::Toleration;
use kube::{Api, Client};

use crate::deployments::Condition;
use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::follow::Follow;
use crate::selector::Selector;
use crate::watch::{Row, namespaced_api};
use crate::workload::{amount, condition, created_unix, describe_selector, images, labels};

/// The fields of a DaemonSet that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DaemonSetRow {
    pub namespace: String,
    pub name: String,
    /// Where it stands, such as `Ready`, `Rolling out` or `Misscheduled`.
    pub status: String,
    /// Nodes that should run its pod.
    pub desired: i32,
    /// Nodes that run its pod.
    pub current: i32,
    pub ready: i32,
    pub updated: i32,
    pub available: i32,
    /// Nodes that run its pod but should not.
    pub misscheduled: i32,
    /// Its pods' node selector, such as `kubernetes.io/os=linux`, or empty.
    pub node_selector: String,
    pub created_unix: Option<i64>,
    /// `None` if it has no selector Taffrail can read.
    pub selector: Option<Selector>,
}

impl DaemonSetRow {
    /// Follows this DaemonSet's pods, so a pod on a new node joins a tail.
    pub fn follow(&self) -> Option<Follow> {
        Some(Follow {
            label: format!("daemonset/{}", self.name),
            namespace: Some(self.namespace.clone()),
            selector: self.selector.clone()?,
        })
    }
}

fn is_on_delete(set: &DaemonSet) -> bool {
    let strategy = set.spec.update_strategy.as_ref();
    strategy.and_then(|strategy| strategy.type_.as_deref()) == Some("OnDelete")
}

/// Where a DaemonSet stands, from its counts of nodes. Problems win over progress, and
/// progress over rest.
fn set_status(set: &DaemonSet) -> &'static str {
    let status = set.status.clone().unwrap_or_default();
    let desired = status.desired_number_scheduled;
    if desired == 0 && status.current_number_scheduled == 0 {
        return "No nodes";
    }
    if desired > 0 && status.number_ready == 0 {
        return "Unavailable";
    }
    if status.number_misscheduled > 0 {
        return "Misscheduled";
    }
    let observed = status.observed_generation.unwrap_or_default();
    let generation = set.metadata.generation.unwrap_or_default();
    let updated = status.updated_number_scheduled.unwrap_or_default();
    if observed < generation || (!is_on_delete(set) && updated < desired) {
        return "Rolling out";
    }
    if updated < desired {
        return "Outdated";
    }
    if status.current_number_scheduled < desired {
        return "Scheduling";
    }
    let available = status.number_available.unwrap_or_default();
    if status.number_ready < desired || available < desired {
        return "Not ready";
    }
    "Ready"
}

fn node_selector(set: &DaemonSet) -> Vec<(String, String)> {
    let spec = set.spec.template.spec.as_ref();
    spec.and_then(|spec| spec.node_selector.clone())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

impl Row for DaemonSetRow {
    type Object = DaemonSet;

    fn from_object(set: &DaemonSet) -> Self {
        let status = set.status.clone().unwrap_or_default();
        let node_selector: Vec<_> = node_selector(set)
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        Self {
            namespace: set.metadata.namespace.clone().unwrap_or_default(),
            name: set.metadata.name.clone().unwrap_or_default(),
            status: set_status(set).to_owned(),
            desired: status.desired_number_scheduled,
            current: status.current_number_scheduled,
            ready: status.number_ready,
            updated: status.updated_number_scheduled.unwrap_or_default(),
            available: status.number_available.unwrap_or_default(),
            misscheduled: status.number_misscheduled,
            node_selector: node_selector.join(","),
            created_unix: created_unix(&set.metadata),
            selector: Selector::from_label_selector(&set.spec.selector).ok(),
        }
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<DaemonSet> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// What the details panel shows for one DaemonSet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonSetDetails {
    /// Such as `RollingUpdate, max surge 0, max unavailable 1`, or `OnDelete`.
    pub strategy: String,
    pub selector: String,
    pub node_selector: Vec<(String, String)>,
    /// Each toleration of its pods, such as `node-role.kubernetes.io/control-plane:NoSchedule`.
    pub tolerations: Vec<String>,
    pub conditions: Vec<Condition>,
    pub images: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

fn describe_strategy(set: &DaemonSet) -> String {
    if is_on_delete(set) {
        return "OnDelete".to_owned();
    }
    let rolling = set
        .spec
        .update_strategy
        .as_ref()
        .and_then(|strategy| strategy.rolling_update.as_ref());
    let surge = amount(rolling.and_then(|rolling| rolling.max_surge.as_ref()), "0");
    let unavailable = amount(
        rolling.and_then(|rolling| rolling.max_unavailable.as_ref()),
        "1",
    );
    format!("RollingUpdate, max surge {surge}, max unavailable {unavailable}")
}

/// A toleration as `key=value:effect` or `key:effect`, or, without a key, as `every taint`
/// or `every NoExecute taint`.
fn describe_toleration(toleration: &Toleration) -> String {
    let key = toleration.key.clone().unwrap_or_default();
    let effect = toleration.effect.clone().unwrap_or_default();
    if key.is_empty() {
        return match effect.is_empty() {
            true => "every taint".to_owned(),
            false => format!("every {effect} taint"),
        };
    }
    let text = match (&toleration.value, toleration.operator.as_deref()) {
        (Some(value), Some("Equal") | None) if !value.is_empty() => format!("{key}={value}"),
        _ => key,
    };
    match effect.is_empty() {
        true => text,
        false => format!("{text}:{effect}"),
    }
}

impl ObjectDetails for DaemonSetDetails {
    type Object = DaemonSet;

    fn api(client: Client, namespace: &str) -> Api<DaemonSet> {
        Api::namespaced(client, namespace)
    }

    fn read(mut set: DaemonSet, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut set)?;
        let spec = &set.spec;
        let conditions = set
            .status
            .iter()
            .flat_map(|status| status.conditions.iter().flatten())
            .map(|c| condition(&c.type_, &c.status, &c.reason, &c.message))
            .collect();
        let tolerations = spec
            .template
            .spec
            .iter()
            .flat_map(|pod| pod.tolerations.iter().flatten())
            .map(describe_toleration)
            .collect();
        Ok(Self {
            strategy: describe_strategy(&set),
            selector: describe_selector(&spec.selector),
            node_selector: node_selector(&set),
            tolerations,
            conditions,
            images: images(&spec.template),
            labels: labels(&set.metadata),
            yaml,
            events,
        })
    }

    fn yaml(&self) -> &str {
        &self.yaml
    }

    fn events(&self) -> &Result<Vec<ObjectEvent>, String> {
        &self.events
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

    fn set(spec: Value, status: Value) -> DaemonSet {
        serde_json::from_value(json!({
            "metadata": {
                "namespace": "kube-system",
                "name": "node-agent",
                "generation": 5,
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "managedFields": [{ "manager": "kubectl" }],
            },
            "spec": spec,
            "status": status,
        }))
        .unwrap()
    }

    fn spec() -> Value {
        json!({
            "selector": { "matchLabels": { "app": "node-agent" } },
            "template": { "spec": {
                "nodeSelector": { "kubernetes.io/os": "linux" },
                "containers": [{ "name": "agent", "image": "agent:4" }],
            } },
        })
    }

    fn settled(nodes: i32) -> Value {
        json!({
            "observedGeneration": 5,
            "desiredNumberScheduled": nodes,
            "currentNumberScheduled": nodes,
            "numberReady": nodes,
            "updatedNumberScheduled": nodes,
            "numberAvailable": nodes,
            "numberMisscheduled": 0,
        })
    }

    fn status_of(spec: Value, status: Value) -> String {
        DaemonSetRow::from_object(&set(spec, status)).status
    }

    #[test]
    fn row_counts_nodes_and_reads_the_selectors() {
        let row = DaemonSetRow::from_object(&set(spec(), settled(4)));

        assert_eq!(row.status, "Ready");
        assert_eq!(
            (
                row.desired,
                row.current,
                row.ready,
                row.updated,
                row.available
            ),
            (4, 4, 4, 4, 4)
        );
        assert_eq!(row.node_selector, "kubernetes.io/os=linux");
        assert_eq!(row.created_unix, Some(1_767_225_600));
        let follow = row.follow().unwrap();
        assert_eq!(follow.label, "daemonset/node-agent");
        assert_eq!(follow.namespace.as_deref(), Some("kube-system"));
        assert_eq!(follow.selector.to_string(), "app=node-agent");
    }

    #[test]
    fn misscheduled_pods_win_over_a_rollout() {
        let mut status = settled(4);
        status["numberMisscheduled"] = json!(1);
        status["updatedNumberScheduled"] = json!(2);
        assert_eq!(status_of(spec(), status), "Misscheduled");
    }

    #[test]
    fn no_ready_pod_is_unavailable_and_some_is_not_ready() {
        let mut status = settled(4);
        status["numberReady"] = json!(0);
        assert_eq!(status_of(spec(), status), "Unavailable");

        let mut status = settled(4);
        status["numberReady"] = json!(3);
        status["numberAvailable"] = json!(3);
        assert_eq!(status_of(spec(), status), "Not ready");
    }

    #[test]
    fn nodes_without_the_update_are_rolling_out_or_outdated() {
        let mut status = settled(4);
        status["updatedNumberScheduled"] = json!(2);
        assert_eq!(status_of(spec(), status.clone()), "Rolling out");

        let mut on_delete = spec();
        on_delete["updateStrategy"] = json!({ "type": "OnDelete" });
        assert_eq!(status_of(on_delete, status), "Outdated");

        let mut status = settled(4);
        status["observedGeneration"] = json!(4);
        assert_eq!(status_of(spec(), status), "Rolling out");
    }

    #[test]
    fn unscheduled_nodes_and_no_nodes_say_so() {
        let mut status = settled(4);
        status["currentNumberScheduled"] = json!(3);
        assert_eq!(status_of(spec(), status), "Scheduling");
        assert_eq!(status_of(spec(), settled(0)), "No nodes");
    }

    #[test]
    fn details_describe_the_strategy_and_where_pods_run() {
        let mut spec = spec();
        spec["updateStrategy"] = json!({
            "type": "RollingUpdate",
            "rollingUpdate": { "maxSurge": 1, "maxUnavailable": "10%" },
        });
        spec["template"]["spec"]["tolerations"] = json!([
            { "key": "node-role.kubernetes.io/control-plane", "operator": "Exists", "effect": "NoSchedule" },
            { "key": "dedicated", "operator": "Equal", "value": "gpu", "effect": "NoExecute" },
            { "operator": "Exists" },
            { "operator": "Exists", "effect": "NoExecute" },
        ]);
        let details = DaemonSetDetails::read(set(spec, settled(4)), Ok(vec![])).unwrap();

        assert_eq!(
            details.strategy,
            "RollingUpdate, max surge 1, max unavailable 10%"
        );
        assert_eq!(details.selector, "app=node-agent");
        assert_eq!(
            details.node_selector,
            [("kubernetes.io/os".to_owned(), "linux".to_owned())]
        );
        assert_eq!(
            details.tolerations,
            [
                "node-role.kubernetes.io/control-plane:NoSchedule",
                "dedicated=gpu:NoExecute",
                "every taint",
                "every NoExecute taint",
            ]
        );
        assert_eq!(details.images, [("agent".to_owned(), "agent:4".to_owned())]);
        assert!(
            details.yaml.contains("name: node-agent"),
            "{}",
            details.yaml
        );
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    #[test]
    fn a_default_strategy_rolls_one_node_at_a_time() {
        let details = DaemonSetDetails::read(set(spec(), settled(1)), Ok(vec![])).unwrap();
        assert_eq!(
            details.strategy,
            "RollingUpdate, max surge 0, max unavailable 1"
        );
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"kube-system","name":"node-agent-x2k","resourceVersion":"1"}}]}"#;
    const SET_LIST: &str = r#"{"kind":"DaemonSetList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"kube-system","name":"node-agent","resourceVersion":"1"},"spec":{"selector":{"matchLabels":{"app":"node-agent"}},"template":{}},"status":{"desiredNumberScheduled":2,"currentNumberScheduled":2,"numberReady":2,"numberMisscheduled":0,"updatedNumberScheduled":2,"numberAvailable":2}}]}"#;

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

    fn serve(sets: Reply) -> fake_api::Handler {
        let sets = Mutex::new(sets);
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
                "/apis/apps/v1/daemonsets" => Some(sets.lock().unwrap().clone()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn daemon_sets_are_watched_like_pods() {
        let connection = fake_api::connect_with(serve(Reply::Json(SET_LIST.into()))).await;
        let mut sets = start::<DaemonSetRow>(connection.client, None);

        let snapshot = settle(&mut sets).await;

        assert_eq!(snapshot.rows[0].name, "node-agent");
        assert_eq!(snapshot.rows[0].desired, 2);
        assert_eq!(snapshot.rows[0].status, "Ready");
    }

    #[tokio::test]
    async fn a_refused_daemon_set_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "daemonsets.apps is forbidden: User \\\"jane\\\" cannot list resource \\\"daemonsets\\\" in API group \\\"apps\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut sets = start::<DaemonSetRow>(connection.client, None);

        let refused = settle(&mut sets).await;
        let listed = settle(&mut pods).await;

        let Some(WatchError::Forbidden { namespace, message }) = &refused.error else {
            panic!("expected a refusal, got {:?}", refused.error);
        };
        assert_eq!(*namespace, None);
        assert!(message.contains("cannot list resource"), "{message}");
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "node-agent-x2k");
    }
}
