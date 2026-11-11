use k8s_openapi::api::core::v1::{Node, NodeCondition};
use kube::{Api, Client};

use crate::deployments::Condition;
use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::watch::Row;

const ROLE_PREFIX: &str = "node-role.kubernetes.io/";
const LEGACY_ROLE: &str = "kubernetes.io/role";
/// Conditions that mark a problem when true. Every other condition but `Ready` is ignored.
const PROBLEMS: [&str; 4] = [
    "MemoryPressure",
    "DiskPressure",
    "PIDPressure",
    "NetworkUnavailable",
];

/// The fields of a node that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeRow {
    pub name: String,
    /// `Ready`, `NotReady` or `Unknown`, then any problems, such as
    /// `Ready, SchedulingDisabled` or `NotReady, DiskPressure`.
    pub status: String,
    /// Such as `control-plane,worker`, from its `node-role.kubernetes.io/*` labels.
    pub roles: String,
    /// The kubelet's version, such as `v1.35.0`.
    pub version: String,
    pub created_unix: Option<i64>,
}

fn condition<'a>(node: &'a Node, kind: &str) -> Option<&'a NodeCondition> {
    node.status
        .as_ref()?
        .conditions
        .as_ref()?
        .iter()
        .find(|condition| condition.type_ == kind)
}

/// Whether the node is ready, then what is wrong with it, as `kubectl get nodes` reads it.
fn node_status(node: &Node) -> String {
    let ready = match condition(node, "Ready").map(|ready| ready.status.as_str()) {
        Some("True") => "Ready",
        Some("False") => "NotReady",
        _ => "Unknown",
    };
    let mut words = vec![ready];
    if node.spec.as_ref().and_then(|spec| spec.unschedulable) == Some(true) {
        words.push("SchedulingDisabled");
    }
    words.extend(
        PROBLEMS
            .into_iter()
            .filter(|kind| condition(node, kind).is_some_and(|c| c.status == "True")),
    );
    words.join(", ")
}

fn roles(node: &Node) -> String {
    let labels = node.metadata.labels.iter().flatten();
    let mut roles: Vec<&str> = labels
        .filter_map(|(key, value)| match key.strip_prefix(ROLE_PREFIX) {
            Some(role) => Some(role),
            None if key == LEGACY_ROLE => Some(value.as_str()),
            None => None,
        })
        .filter(|role| !role.is_empty())
        .collect();
    roles.sort_unstable();
    roles.dedup();
    roles.join(",")
}

impl Row for NodeRow {
    type Object = Node;

    const NAMESPACED: bool = false;

    fn from_object(node: &Node) -> Self {
        Self {
            name: node.metadata.name.clone().unwrap_or_default(),
            status: node_status(node),
            roles: roles(node),
            version: node
                .status
                .as_ref()
                .and_then(|status| status.node_info.as_ref())
                .map(|info| info.kubelet_version.clone())
                .unwrap_or_default(),
            created_unix: node
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|created| created.0.as_second()),
        }
    }

    fn api(client: Client, _: Option<&str>) -> Api<Node> {
        Api::all(client)
    }

    fn namespace(&self) -> &str {
        ""
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// What the details panel shows for one node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeDetails {
    /// Each address and its type, such as `InternalIP` and `10.0.7.4`.
    pub addresses: Vec<(String, String)>,
    /// Each resource, with its capacity and what pods may use, such as `cpu`, `8`, `7910m`.
    pub resources: Vec<(String, String, String)>,
    /// Such as `Ubuntu 24.04.1 LTS`.
    pub os_image: String,
    /// Such as `linux/arm64`.
    pub platform: String,
    pub kernel: String,
    /// Such as `containerd://2.0.1`.
    pub runtime: String,
    /// Such as `node-role.kubernetes.io/control-plane:NoSchedule`.
    pub taints: Vec<String>,
    pub conditions: Vec<Condition>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

impl ObjectDetails for NodeDetails {
    type Object = Node;

    fn api(client: Client, _: &str) -> Api<Node> {
        Api::all(client)
    }

    fn read(mut node: Node, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut node)?;
        let status = node.status.clone().unwrap_or_default();
        let info = status.node_info.clone().unwrap_or_default();
        let capacity = status.capacity.clone().unwrap_or_default();
        let allocatable = status.allocatable.clone().unwrap_or_default();
        let resources = capacity
            .iter()
            .map(|(resource, quantity)| {
                let usable = allocatable.get(resource).map(|usable| usable.0.clone());
                (
                    resource.clone(),
                    quantity.0.clone(),
                    usable.unwrap_or_default(),
                )
            })
            .collect();
        let taints = node
            .spec
            .iter()
            .flat_map(|spec| spec.taints.iter().flatten())
            .map(|taint| match &taint.value {
                Some(value) if !value.is_empty() => {
                    format!("{}={value}:{}", taint.key, taint.effect)
                }
                _ => format!("{}:{}", taint.key, taint.effect),
            })
            .collect();
        let platform = match (&*info.operating_system, &*info.architecture) {
            ("", "") => String::new(),
            (os, arch) => format!("{os}/{arch}"),
        };
        Ok(Self {
            addresses: status
                .addresses
                .iter()
                .flatten()
                .map(|address| (address.type_.clone(), address.address.clone()))
                .collect(),
            resources,
            os_image: info.os_image,
            platform,
            kernel: info.kernel_version,
            runtime: info.container_runtime_version,
            taints,
            conditions: status
                .conditions
                .iter()
                .flatten()
                .map(|condition| Condition {
                    kind: condition.type_.clone(),
                    status: condition.status.clone(),
                    reason: condition.reason.clone().unwrap_or_default(),
                    message: condition.message.clone().unwrap_or_default(),
                })
                .collect(),
            labels: node
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

    fn node(labels: Value, spec: Value, conditions: Value) -> Node {
        serde_json::from_value(json!({
            "metadata": {
                "name": "node-1",
                "labels": labels,
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "managedFields": [{ "manager": "kubelet" }],
            },
            "spec": spec,
            "status": {
                "conditions": conditions,
                "addresses": [
                    { "type": "InternalIP", "address": "10.0.7.4" },
                    { "type": "Hostname", "address": "node-1" },
                ],
                "capacity": { "cpu": "8", "memory": "32Gi", "pods": "110" },
                "allocatable": { "cpu": "7910m", "memory": "31Gi", "pods": "110" },
                "nodeInfo": {
                    "architecture": "arm64", "bootID": "", "containerRuntimeVersion": "containerd://2.0.1",
                    "kernelVersion": "6.8.0", "kubeProxyVersion": "", "kubeletVersion": "v1.35.0",
                    "machineID": "", "operatingSystem": "linux", "osImage": "Ubuntu 24.04.1 LTS",
                    "systemUUID": "",
                },
            },
        }))
        .unwrap()
    }

    fn ready(status: &str) -> Value {
        json!([
            { "type": "MemoryPressure", "status": "False" },
            { "type": "Ready", "status": status, "reason": "KubeletReady" },
        ])
    }

    fn status_of(spec: Value, conditions: Value) -> String {
        NodeRow::from_object(&node(json!({}), spec, conditions)).status
    }

    #[test]
    fn row_reads_status_roles_and_version() {
        let labels = json!({
            "node-role.kubernetes.io/control-plane": "",
            "node-role.kubernetes.io/worker": "true",
            "kubernetes.io/hostname": "node-1",
        });
        let row = NodeRow::from_object(&node(labels, json!({}), ready("True")));

        assert_eq!(row.name, "node-1");
        assert_eq!(row.namespace(), "");
        assert_eq!(row.status, "Ready");
        assert_eq!(row.roles, "control-plane,worker");
        assert_eq!(row.version, "v1.35.0");
        assert_eq!(row.created_unix, Some(1_767_225_600));
    }

    #[test]
    fn the_legacy_role_label_counts_too() {
        let labels = json!({ "kubernetes.io/role": "master" });
        let row = NodeRow::from_object(&node(labels, json!({}), ready("True")));
        assert_eq!(row.roles, "master");
    }

    #[test]
    fn readiness_comes_from_the_ready_condition() {
        assert_eq!(status_of(json!({}), ready("False")), "NotReady");
        assert_eq!(status_of(json!({}), ready("Unknown")), "Unknown");
        assert_eq!(status_of(json!({}), json!([])), "Unknown");
    }

    #[test]
    fn a_cordoned_node_says_scheduling_is_disabled() {
        let spec = json!({ "unschedulable": true });
        assert_eq!(status_of(spec, ready("True")), "Ready, SchedulingDisabled");
    }

    #[test]
    fn pressure_conditions_follow_readiness() {
        let conditions = json!([
            { "type": "Ready", "status": "False" },
            { "type": "DiskPressure", "status": "True" },
            { "type": "PIDPressure", "status": "True" },
        ]);
        assert_eq!(
            status_of(json!({}), conditions),
            "NotReady, DiskPressure, PIDPressure"
        );
    }

    #[test]
    fn details_describe_addresses_resources_and_taints() {
        let spec = json!({ "taints": [
            { "key": "node-role.kubernetes.io/control-plane", "effect": "NoSchedule" },
            { "key": "dedicated", "value": "gpu", "effect": "NoExecute" },
        ]});
        let details = NodeDetails::read(node(json!({}), spec, ready("True")), Ok(vec![])).unwrap();

        assert_eq!(
            details.addresses[0],
            ("InternalIP".to_owned(), "10.0.7.4".to_owned())
        );
        assert!(details.resources.contains(&(
            "cpu".to_owned(),
            "8".to_owned(),
            "7910m".to_owned()
        )));
        assert_eq!(details.platform, "linux/arm64");
        assert_eq!(details.os_image, "Ubuntu 24.04.1 LTS");
        assert_eq!(details.runtime, "containerd://2.0.1");
        assert_eq!(
            details.taints,
            [
                "node-role.kubernetes.io/control-plane:NoSchedule",
                "dedicated=gpu:NoExecute"
            ]
        );
        assert_eq!(details.conditions[1].kind, "Ready");
        assert!(details.yaml.contains("name: node-1"), "{}", details.yaml);
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"}}]}"#;
    const NODE_LIST: &str = r#"{"kind":"NodeList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"node-1","resourceVersion":"1"},"status":{"conditions":[{"type":"Ready","status":"True"}]}}]}"#;
    const NODE: &str =
        r#"{"kind":"Node","apiVersion":"v1","metadata":{"name":"node-1","resourceVersion":"1"}}"#;
    const NODE_EVENTS: &str = r#"{"kind":"EventList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"node-1.1","namespace":"default"},"involvedObject":{"kind":"Node","name":"node-1"},"type":"Normal","reason":"NodeReady","message":"Node node-1 status is now: NodeReady","count":1}]}"#;

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

    /// Answers the pod list, the node list with `nodes`, and records the event lists asked for.
    fn serve(nodes: Reply, asked: Arc<Mutex<Vec<Request>>>) -> fake_api::Handler {
        let nodes = Mutex::new(nodes);
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
                "/api/v1/nodes" => Some(nodes.lock().unwrap().clone()),
                "/api/v1/nodes/node-1" => Some(Reply::Json(NODE.into())),
                "/api/v1/events" => {
                    asked.lock().unwrap().push(request.clone());
                    Some(Reply::Json(NODE_EVENTS.into()))
                }
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn nodes_are_watched_across_the_cluster_whatever_the_namespace() {
        let asked = Arc::default();
        let connection = fake_api::connect_with(serve(Reply::Json(NODE_LIST.into()), asked)).await;
        let mut nodes = start::<NodeRow>(connection.client, Some("shop".into()));

        let snapshot = settle(&mut nodes).await;

        assert_eq!(snapshot.rows[0].name, "node-1");
        assert_eq!(snapshot.rows[0].status, "Ready");
    }

    #[tokio::test]
    async fn a_refused_node_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(
            Reply::Forbidden(
                "nodes is forbidden: User \\\"jane\\\" cannot list resource \\\"nodes\\\" in API group \\\"\\\" at the cluster scope",
            ),
            Arc::default(),
        ))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut nodes = start::<NodeRow>(connection.client, None);

        let refused = settle(&mut nodes).await;
        let listed = settle(&mut pods).await;

        let Some(WatchError::Forbidden { namespace, message }) = &refused.error else {
            panic!("expected a refusal, got {:?}", refused.error);
        };
        assert_eq!(*namespace, None);
        assert!(message.contains("cannot list resource"), "{message}");
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "api-1");
    }

    #[tokio::test]
    async fn a_nodes_events_are_read_from_every_namespace() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let connection =
            fake_api::connect_with(serve(Reply::Json(NODE_LIST.into()), asked.clone())).await;

        let details =
            crate::details::load::<NodeDetails>(connection.client, String::new(), "node-1".into())
                .await
                .unwrap();

        let events = details.events.unwrap();
        assert_eq!(events[0].reason, "NodeReady");
        let asked = asked.lock().unwrap();
        assert_eq!(
            asked[0].query["fieldSelector"],
            "involvedObject.kind=Node,involvedObject.name=node-1"
        );
    }
}
