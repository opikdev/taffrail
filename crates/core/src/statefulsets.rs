use k8s_openapi::api::apps::v1::StatefulSet;
use kube::{Api, Client};

use crate::deployments::Condition;
use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::follow::Follow;
use crate::selector::Selector;
use crate::watch::{Row, namespaced_api};
use crate::workload::{amount, condition, created_unix, describe_selector, images, labels};

/// The fields of a StatefulSet that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatefulSetRow {
    pub namespace: String,
    pub name: String,
    /// Where it stands, such as `Ready`, `Rolling out` or `Unavailable`.
    pub status: String,
    pub desired: i32,
    pub ready: i32,
    pub updated: i32,
    pub created_unix: Option<i64>,
    /// `None` if it has no selector Taffrail can read.
    pub selector: Option<Selector>,
}

impl StatefulSetRow {
    /// Follows this StatefulSet's pods, so a pod that is replaced joins a tail.
    pub fn follow(&self) -> Option<Follow> {
        Some(Follow {
            label: format!("statefulset/{}", self.name),
            namespace: Some(self.namespace.clone()),
            selector: self.selector.clone()?,
        })
    }
}

fn is_on_delete(set: &StatefulSet) -> bool {
    let strategy = set.spec.update_strategy.as_ref();
    strategy.and_then(|strategy| strategy.type_.as_deref()) == Some("OnDelete")
}

fn partition(set: &StatefulSet) -> i32 {
    set.spec
        .update_strategy
        .as_ref()
        .and_then(|strategy| strategy.rolling_update.as_ref())
        .and_then(|rolling| rolling.partition)
        .unwrap_or_default()
}

/// Where a StatefulSet stands, from its replica counts and revisions. Problems win over
/// progress, and progress over rest.
fn set_status(set: &StatefulSet) -> &'static str {
    let status = set.status.clone().unwrap_or_default();
    let desired = set.spec.replicas.unwrap_or(1);
    let ready = status.ready_replicas.unwrap_or_default();
    if desired == 0 && status.replicas == 0 {
        return "ScaledToZero";
    }
    if desired > 0 && ready == 0 {
        return "Unavailable";
    }
    let observed = status.observed_generation.unwrap_or_default();
    let generation = set.metadata.generation.unwrap_or_default();
    let revisions_differ = status.update_revision.is_some()
        && status.current_revision.is_some()
        && status.update_revision != status.current_revision;
    let updated = status.updated_replicas.unwrap_or_default();
    // Pods below the partition keep the old revision on purpose, so the rollout ends once
    // the pods above it are updated.
    let partition = partition(set);
    let rolling = !is_on_delete(set)
        && match partition {
            0 => revisions_differ || updated < desired,
            _ => updated < desired - partition.min(desired),
        };
    if observed < generation || rolling {
        return "Rolling out";
    }
    if is_on_delete(set) && revisions_differ {
        return "Outdated";
    }
    if status.replicas != desired {
        return "Scaling";
    }
    if ready < desired {
        return "Not ready";
    }
    "Ready"
}

impl Row for StatefulSetRow {
    type Object = StatefulSet;

    fn from_object(set: &StatefulSet) -> Self {
        let status = set.status.as_ref();
        Self {
            namespace: set.metadata.namespace.clone().unwrap_or_default(),
            name: set.metadata.name.clone().unwrap_or_default(),
            status: set_status(set).to_owned(),
            desired: set.spec.replicas.unwrap_or(1),
            ready: status
                .and_then(|status| status.ready_replicas)
                .unwrap_or_default(),
            updated: status
                .and_then(|status| status.updated_replicas)
                .unwrap_or_default(),
            created_unix: created_unix(&set.metadata),
            selector: Selector::from_label_selector(&set.spec.selector).ok(),
        }
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<StatefulSet> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A persistent volume claim each pod of a StatefulSet gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimTemplate {
    pub name: String,
    /// The storage it requests, such as `10Gi`, or empty.
    pub storage: String,
    /// Its storage class, or empty for the cluster's default.
    pub class: String,
}

/// What the details panel shows for one StatefulSet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatefulSetDetails {
    /// Such as `RollingUpdate, partition 0, max unavailable 1`, or `OnDelete`.
    pub strategy: String,
    /// `OrderedReady` or `Parallel`.
    pub pod_management: String,
    /// The headless Service that names its pods, or empty.
    pub service: String,
    pub current_replicas: i32,
    pub current_revision: String,
    pub update_revision: String,
    pub selector: String,
    pub conditions: Vec<Condition>,
    pub images: Vec<(String, String)>,
    pub claims: Vec<ClaimTemplate>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

fn describe_strategy(set: &StatefulSet) -> String {
    if is_on_delete(set) {
        return "OnDelete".to_owned();
    }
    let rolling = set
        .spec
        .update_strategy
        .as_ref()
        .and_then(|strategy| strategy.rolling_update.as_ref());
    let unavailable = amount(
        rolling.and_then(|rolling| rolling.max_unavailable.as_ref()),
        "1",
    );
    format!(
        "RollingUpdate, partition {}, max unavailable {unavailable}",
        partition(set)
    )
}

impl ObjectDetails for StatefulSetDetails {
    type Object = StatefulSet;

    fn api(client: Client, namespace: &str) -> Api<StatefulSet> {
        Api::namespaced(client, namespace)
    }

    fn read(
        mut set: StatefulSet,
        events: Result<Vec<ObjectEvent>, String>,
    ) -> Result<Self, String> {
        let yaml = to_yaml(&mut set)?;
        let spec = &set.spec;
        let status = set.status.clone().unwrap_or_default();
        let conditions = status
            .conditions
            .iter()
            .flatten()
            .map(|c| condition(&c.type_, &c.status, &c.reason, &c.message))
            .collect();
        let claims = spec
            .volume_claim_templates
            .iter()
            .flatten()
            .map(|claim| {
                let spec = claim.spec.as_ref();
                let storage = spec
                    .and_then(|spec| spec.resources.as_ref()?.requests.as_ref()?.get("storage"))
                    .map(|quantity| quantity.0.clone());
                ClaimTemplate {
                    name: claim.metadata.name.clone().unwrap_or_default(),
                    storage: storage.unwrap_or_default(),
                    class: spec
                        .and_then(|spec| spec.storage_class_name.clone())
                        .unwrap_or_default(),
                }
            })
            .collect();
        Ok(Self {
            strategy: describe_strategy(&set),
            pod_management: spec
                .pod_management_policy
                .clone()
                .unwrap_or_else(|| "OrderedReady".to_owned()),
            service: spec.service_name.clone().unwrap_or_default(),
            current_replicas: status.current_replicas.unwrap_or_default(),
            current_revision: status.current_revision.clone().unwrap_or_default(),
            update_revision: status.update_revision.clone().unwrap_or_default(),
            selector: describe_selector(&spec.selector),
            conditions,
            images: images(&spec.template),
            claims,
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

    fn set(spec: Value, status: Value) -> StatefulSet {
        serde_json::from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "db",
                "generation": 2,
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "managedFields": [{ "manager": "kubectl" }],
            },
            "spec": spec,
            "status": status,
        }))
        .unwrap()
    }

    fn spec(replicas: i32) -> Value {
        json!({
            "replicas": replicas,
            "serviceName": "db",
            "selector": { "matchLabels": { "app": "db" } },
            "template": { "spec": { "containers": [{ "name": "postgres", "image": "postgres:17" }] } },
        })
    }

    fn settled(replicas: i32) -> Value {
        json!({
            "observedGeneration": 2,
            "replicas": replicas,
            "readyReplicas": replicas,
            "currentReplicas": replicas,
            "updatedReplicas": replicas,
            "availableReplicas": replicas,
            "currentRevision": "db-6c8f",
            "updateRevision": "db-6c8f",
        })
    }

    fn status_of(spec: Value, status: Value) -> String {
        StatefulSetRow::from_object(&set(spec, status)).status
    }

    #[test]
    fn row_counts_replicas_and_reads_the_selector() {
        let row = StatefulSetRow::from_object(&set(spec(3), settled(3)));

        assert_eq!(row.status, "Ready");
        assert_eq!((row.desired, row.ready, row.updated), (3, 3, 3));
        assert_eq!(row.created_unix, Some(1_767_225_600));
        let follow = row.follow().unwrap();
        assert_eq!(follow.label, "statefulset/db");
        assert_eq!(follow.namespace.as_deref(), Some("shop"));
        assert_eq!(follow.selector.to_string(), "app=db");
    }

    #[test]
    fn a_new_revision_that_has_not_reached_every_pod_is_rolling_out() {
        let mut status = settled(3);
        status["updateRevision"] = json!("db-7d9a");
        status["updatedReplicas"] = json!(1);
        assert_eq!(status_of(spec(3), status), "Rolling out");

        let mut status = settled(3);
        status["observedGeneration"] = json!(1);
        assert_eq!(status_of(spec(3), status), "Rolling out");
    }

    #[test]
    fn a_partitioned_rollout_ends_at_the_partition() {
        let mut spec = spec(3);
        spec["updateStrategy"] =
            json!({ "type": "RollingUpdate", "rollingUpdate": { "partition": 2 } });
        let mut status = settled(3);
        status["updateRevision"] = json!("db-7d9a");
        status["updatedReplicas"] = json!(1);
        assert_eq!(status_of(spec.clone(), status.clone()), "Ready");

        status["updatedReplicas"] = json!(0);
        assert_eq!(status_of(spec, status), "Rolling out");
    }

    #[test]
    fn on_delete_pods_with_the_old_revision_are_outdated() {
        let mut spec = spec(3);
        spec["updateStrategy"] = json!({ "type": "OnDelete" });
        let mut status = settled(3);
        status["updateRevision"] = json!("db-7d9a");
        status["updatedReplicas"] = json!(0);
        assert_eq!(status_of(spec, status), "Outdated");
    }

    #[test]
    fn no_ready_pods_is_unavailable_and_some_is_not_ready() {
        let mut status = settled(3);
        status["readyReplicas"] = json!(0);
        assert_eq!(status_of(spec(3), status), "Unavailable");

        let mut status = settled(3);
        status["readyReplicas"] = json!(2);
        assert_eq!(status_of(spec(3), status), "Not ready");
    }

    #[test]
    fn scaling_and_scaled_down_sets_say_so() {
        let mut status = settled(3);
        status["replicas"] = json!(4);
        assert_eq!(status_of(spec(3), status), "Scaling");
        assert_eq!(status_of(spec(0), settled(0)), "ScaledToZero");
        assert_eq!(status_of(spec(0), settled(2)), "Scaling");
    }

    #[test]
    fn details_describe_the_strategy_revisions_and_claims() {
        let mut spec = spec(3);
        spec["podManagementPolicy"] = json!("Parallel");
        spec["updateStrategy"] = json!({
            "type": "RollingUpdate",
            "rollingUpdate": { "partition": 1, "maxUnavailable": "50%" },
        });
        spec["volumeClaimTemplates"] = json!([{
            "metadata": { "name": "data" },
            "spec": { "storageClassName": "fast", "resources": { "requests": { "storage": "10Gi" } } },
        }]);
        let mut status = settled(3);
        status["updateRevision"] = json!("db-7d9a");
        status["conditions"] = json!([{ "type": "Healthy", "status": "True", "reason": "Fine" }]);
        let details = StatefulSetDetails::read(set(spec, status), Ok(vec![])).unwrap();

        assert_eq!(
            details.strategy,
            "RollingUpdate, partition 1, max unavailable 50%"
        );
        assert_eq!(details.pod_management, "Parallel");
        assert_eq!(details.service, "db");
        assert_eq!(details.current_revision, "db-6c8f");
        assert_eq!(details.update_revision, "db-7d9a");
        assert_eq!(details.selector, "app=db");
        assert_eq!(
            details.images,
            [("postgres".to_owned(), "postgres:17".to_owned())]
        );
        assert_eq!(
            details.claims,
            [ClaimTemplate {
                name: "data".into(),
                storage: "10Gi".into(),
                class: "fast".into(),
            }]
        );
        assert_eq!(details.conditions[0].reason, "Fine");
        assert!(details.yaml.contains("name: db"), "{}", details.yaml);
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    #[test]
    fn an_on_delete_strategy_has_no_partition() {
        let mut spec = spec(1);
        spec["updateStrategy"] = json!({ "type": "OnDelete" });
        let details = StatefulSetDetails::read(set(spec, settled(1)), Ok(vec![])).unwrap();
        assert_eq!(details.strategy, "OnDelete");
        assert_eq!(details.pod_management, "OrderedReady");
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"db-0","resourceVersion":"1"}}]}"#;
    const SET_LIST: &str = r#"{"kind":"StatefulSetList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"db","resourceVersion":"1"},"spec":{"replicas":1,"serviceName":"db","selector":{"matchLabels":{"app":"db"}},"template":{}}}]}"#;

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
                "/apis/apps/v1/statefulsets" => Some(sets.lock().unwrap().clone()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn stateful_sets_are_watched_like_pods() {
        let connection = fake_api::connect_with(serve(Reply::Json(SET_LIST.into()))).await;
        let mut sets = start::<StatefulSetRow>(connection.client, None);

        let snapshot = settle(&mut sets).await;

        assert_eq!(snapshot.rows[0].name, "db");
        assert_eq!(snapshot.rows[0].desired, 1);
        assert_eq!(snapshot.rows[0].status, "Unavailable");
    }

    #[tokio::test]
    async fn a_refused_stateful_set_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "statefulsets.apps is forbidden: User \\\"jane\\\" cannot list resource \\\"statefulsets\\\" in API group \\\"apps\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut sets = start::<StatefulSetRow>(connection.client, None);

        let refused = settle(&mut sets).await;
        let listed = settle(&mut pods).await;

        let Some(WatchError::Forbidden { namespace, message }) = &refused.error else {
            panic!("expected a refusal, got {:?}", refused.error);
        };
        assert_eq!(*namespace, None);
        assert!(message.contains("cannot list resource"), "{message}");
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "db-0");
    }
}
