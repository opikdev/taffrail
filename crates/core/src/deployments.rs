use k8s_openapi::api::apps::v1::{Deployment, DeploymentCondition};
use kube::{Api, Client};

use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::follow::Follow;
use crate::selector::Selector;
use crate::watch::{Row, namespaced_api};

/// The fields of a Deployment that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeploymentRow {
    pub namespace: String,
    pub name: String,
    /// Where its rollout stands, such as `Available`, `Progressing` or `Stalled`.
    pub status: String,
    pub desired: i32,
    pub ready: i32,
    pub updated: i32,
    pub available: i32,
    pub created_unix: Option<i64>,
    /// `None` if it has no selector Taffrail can read.
    pub selector: Option<Selector>,
}

impl DeploymentRow {
    /// Follows this Deployment's pods, so a rollout's new pods join a tail.
    pub fn follow(&self) -> Option<Follow> {
        Some(Follow {
            label: format!("deployment/{}", self.name),
            namespace: Some(self.namespace.clone()),
            selector: self.selector.clone()?,
        })
    }
}

fn condition<'a>(deployment: &'a Deployment, kind: &str) -> Option<&'a DeploymentCondition> {
    deployment
        .status
        .as_ref()?
        .conditions
        .as_ref()?
        .iter()
        .find(|condition| condition.type_.as_deref() == Some(kind))
}

fn is(condition: Option<&DeploymentCondition>, status: &str) -> bool {
    condition.is_some_and(|condition| condition.status.as_deref() == Some(status))
}

/// Where a rollout stands, from the Deployment's conditions and replica counts. Problems
/// win over progress, and progress over rest.
fn rollout_status(deployment: &Deployment) -> &'static str {
    let spec = &deployment.spec;
    let status = deployment.status.clone().unwrap_or_default();
    let desired = spec.replicas.unwrap_or(1);
    let progressing = condition(deployment, "Progressing");
    let stalled = is(progressing, "False")
        && progressing.and_then(|condition| condition.reason.as_deref())
            == Some("ProgressDeadlineExceeded");
    if stalled {
        return "Stalled";
    }
    if is(condition(deployment, "ReplicaFailure"), "True") {
        return "ReplicaFailure";
    }
    if spec.paused == Some(true) {
        return "Paused";
    }
    if desired > 0 && is(condition(deployment, "Available"), "False") {
        return "Unavailable";
    }
    let observed = status.observed_generation.unwrap_or_default();
    let generation = deployment.metadata.generation.unwrap_or_default();
    let updated = status.updated_replicas.unwrap_or_default();
    let rolling = observed < generation
        || updated < desired
        || status.replicas.unwrap_or_default() > updated
        || status.available_replicas.unwrap_or_default() < desired;
    match (rolling, desired) {
        (true, _) => "Progressing",
        (false, 0) => "ScaledToZero",
        (false, _) => "Available",
    }
}

impl Row for DeploymentRow {
    type Object = Deployment;

    fn from_object(deployment: &Deployment) -> Self {
        let status = deployment.status.as_ref();
        let count = |field: fn(&k8s_openapi::api::apps::v1::DeploymentStatus) -> Option<i32>| {
            status.and_then(field).unwrap_or_default()
        };
        Self {
            namespace: deployment.metadata.namespace.clone().unwrap_or_default(),
            name: deployment.metadata.name.clone().unwrap_or_default(),
            status: rollout_status(deployment).to_owned(),
            desired: deployment.spec.replicas.unwrap_or(1),
            ready: count(|status| status.ready_replicas),
            updated: count(|status| status.updated_replicas),
            available: count(|status| status.available_replicas),
            created_unix: deployment
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|created| created.0.as_second()),
            selector: Selector::from_label_selector(&deployment.spec.selector).ok(),
        }
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<Deployment> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// One of a Deployment's conditions, such as `Available` or `Progressing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub kind: String,
    /// `True`, `False` or `Unknown`.
    pub status: String,
    pub reason: String,
    pub message: String,
}

/// What the details panel shows for one Deployment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentDetails {
    /// Such as `RollingUpdate, max surge 25%, max unavailable 25%`.
    pub strategy: String,
    pub selector: String,
    pub conditions: Vec<Condition>,
    /// Each container of its pod template, and its image.
    pub images: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

fn describe_strategy(deployment: &Deployment) -> String {
    let strategy = deployment.spec.strategy.as_ref();
    let kind = strategy
        .and_then(|strategy| strategy.type_.clone())
        .unwrap_or_else(|| "RollingUpdate".to_owned());
    let rolling = strategy.and_then(|strategy| strategy.rolling_update.as_ref());
    if kind != "RollingUpdate" {
        return kind;
    }
    let amount = |value: Option<&k8s_openapi::apimachinery::pkg::util::intstr::IntOrString>| {
        use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
        match value {
            Some(IntOrString::Int(count)) => count.to_string(),
            Some(IntOrString::String(text)) => text.clone(),
            None => "25%".to_owned(),
        }
    };
    let surge = amount(rolling.and_then(|rolling| rolling.max_surge.as_ref()));
    let unavailable = amount(rolling.and_then(|rolling| rolling.max_unavailable.as_ref()));
    format!("{kind}, max surge {surge}, max unavailable {unavailable}")
}

impl ObjectDetails for DeploymentDetails {
    type Object = Deployment;

    fn api(client: Client, namespace: &str) -> Api<Deployment> {
        Api::namespaced(client, namespace)
    }

    fn read(
        mut deployment: Deployment,
        events: Result<Vec<ObjectEvent>, String>,
    ) -> Result<Self, String> {
        let yaml = to_yaml(&mut deployment)?;
        let spec = &deployment.spec;
        let selector = match Selector::from_label_selector(&spec.selector) {
            Ok(selector) => selector.to_string(),
            Err(error) => error,
        };
        let conditions = deployment
            .status
            .iter()
            .flat_map(|status| status.conditions.iter().flatten())
            .map(|condition| Condition {
                kind: condition.type_.clone().unwrap_or_default(),
                status: condition.status.clone().unwrap_or_default(),
                reason: condition.reason.clone().unwrap_or_default(),
                message: condition.message.clone().unwrap_or_default(),
            })
            .collect();
        let images = spec
            .template
            .spec
            .iter()
            .flat_map(|pod| &pod.containers)
            .map(|container| {
                let image = container.image.clone().unwrap_or_default();
                (container.name.clone(), image)
            })
            .collect();
        Ok(Self {
            strategy: describe_strategy(&deployment),
            selector,
            conditions,
            images,
            labels: deployment
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

    fn deployment(spec: Value, status: Value) -> Deployment {
        serde_json::from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "api",
                "generation": 4,
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
            "selector": { "matchLabels": { "app": "api" } },
            "template": { "spec": { "containers": [{ "name": "app", "image": "api:2" }] } },
        })
    }

    fn settled(replicas: i32) -> Value {
        json!({
            "observedGeneration": 4,
            "replicas": replicas,
            "readyReplicas": replicas,
            "updatedReplicas": replicas,
            "availableReplicas": replicas,
            "conditions": [
                { "type": "Available", "status": "True", "reason": "MinimumReplicasAvailable" },
                { "type": "Progressing", "status": "True", "reason": "NewReplicaSetAvailable" },
            ],
        })
    }

    fn status_of(spec: Value, status: Value) -> String {
        DeploymentRow::from_object(&deployment(spec, status)).status
    }

    #[test]
    fn row_counts_replicas_and_reads_the_selector() {
        let row = DeploymentRow::from_object(&deployment(spec(3), settled(3)));

        assert_eq!(row.status, "Available");
        assert_eq!(
            (row.desired, row.ready, row.updated, row.available),
            (3, 3, 3, 3)
        );
        assert_eq!(row.created_unix, Some(1_767_225_600));
        let follow = row.follow().unwrap();
        assert_eq!(follow.label, "deployment/api");
        assert_eq!(follow.namespace.as_deref(), Some("shop"));
        assert_eq!(follow.selector.to_string(), "app=api");
    }

    #[test]
    fn a_rollout_that_has_not_finished_is_progressing() {
        let mut status = settled(3);
        status["updatedReplicas"] = json!(1);
        assert_eq!(status_of(spec(3), status), "Progressing");

        let mut status = settled(3);
        status["observedGeneration"] = json!(3);
        assert_eq!(status_of(spec(3), status), "Progressing");
    }

    #[test]
    fn a_rollout_past_its_deadline_is_stalled() {
        let mut status = settled(3);
        status["updatedReplicas"] = json!(1);
        status["conditions"][1] = json!({
            "type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded",
        });
        assert_eq!(status_of(spec(3), status), "Stalled");
    }

    #[test]
    fn missing_minimum_availability_is_unavailable() {
        let mut status = settled(3);
        status["availableReplicas"] = json!(0);
        status["conditions"][0] = json!({
            "type": "Available", "status": "False", "reason": "MinimumReplicasUnavailable",
        });
        assert_eq!(status_of(spec(3), status), "Unavailable");
    }

    #[test]
    fn a_replica_failure_wins_over_progress() {
        let mut status = settled(3);
        status["updatedReplicas"] = json!(0);
        status["conditions"] = json!([{
            "type": "ReplicaFailure", "status": "True", "reason": "FailedCreate",
        }]);
        assert_eq!(status_of(spec(3), status), "ReplicaFailure");
    }

    #[test]
    fn paused_and_scaled_down_deployments_say_so() {
        let mut paused = spec(3);
        paused["paused"] = json!(true);
        assert_eq!(status_of(paused, settled(3)), "Paused");
        assert_eq!(status_of(spec(0), settled(0)), "ScaledToZero");
    }

    #[test]
    fn details_describe_the_strategy_conditions_and_images() {
        let mut spec = spec(3);
        spec["strategy"] = json!({
            "type": "RollingUpdate",
            "rollingUpdate": { "maxSurge": 1, "maxUnavailable": "10%" },
        });
        let details = DeploymentDetails::read(deployment(spec, settled(3)), Ok(vec![])).unwrap();

        assert_eq!(
            details.strategy,
            "RollingUpdate, max surge 1, max unavailable 10%"
        );
        assert_eq!(details.selector, "app=api");
        assert_eq!(details.images, [("app".to_owned(), "api:2".to_owned())]);
        assert_eq!(details.conditions[0].kind, "Available");
        assert_eq!(details.conditions[0].reason, "MinimumReplicasAvailable");
        assert!(details.yaml.contains("kind: Deployment") || details.yaml.contains("name: api"));
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    #[test]
    fn a_recreate_strategy_has_no_surge() {
        let mut spec = spec(1);
        spec["strategy"] = json!({ "type": "Recreate" });
        let details = DeploymentDetails::read(deployment(spec, settled(1)), Ok(vec![])).unwrap();
        assert_eq!(details.strategy, "Recreate");
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"}}]}"#;
    const DEPLOYMENT_LIST: &str = r#"{"kind":"DeploymentList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api","resourceVersion":"1"},"spec":{"replicas":1,"selector":{"matchLabels":{"app":"api"}},"template":{}}}]}"#;

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

    fn serve(deployments: Reply) -> fake_api::Handler {
        let deployments = Mutex::new(deployments);
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
                "/apis/apps/v1/deployments" => Some(deployments.lock().unwrap().clone()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn deployments_are_watched_like_pods() {
        let connection = fake_api::connect_with(serve(Reply::Json(DEPLOYMENT_LIST.into()))).await;
        let mut deployments = start::<DeploymentRow>(connection.client, None);

        let snapshot = settle(&mut deployments).await;

        assert_eq!(snapshot.rows[0].name, "api");
        assert_eq!(snapshot.rows[0].desired, 1);
    }

    #[tokio::test]
    async fn a_refused_deployment_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "deployments.apps is forbidden: User \\\"jane\\\" cannot list resource \\\"deployments\\\" in API group \\\"apps\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut deployments = start::<DeploymentRow>(connection.client, None);

        let refused = settle(&mut deployments).await;
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
    async fn a_kind_the_cluster_does_not_serve_says_so() {
        let connection = fake_api::connect_with(serve(Reply::Status(404))).await;
        let mut deployments = start::<DeploymentRow>(connection.client, None);

        let snapshot = settle(&mut deployments).await;

        assert!(
            matches!(snapshot.error, Some(WatchError::NotServed { .. })),
            "{:?}",
            snapshot.error
        );
    }
}
