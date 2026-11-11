use k8s_openapi::api::core::v1::Namespace;
use kube::{Api, Client};

use crate::deployments::Condition;
use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::watch::Row;

/// The fields of a namespace that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamespaceRow {
    pub name: String,
    /// Its phase: `Active` or `Terminating`.
    pub status: String,
    pub created_unix: Option<i64>,
}

impl Row for NamespaceRow {
    type Object = Namespace;

    const NAMESPACED: bool = false;

    fn from_object(namespace: &Namespace) -> Self {
        let deleting = namespace.metadata.deletion_timestamp.is_some();
        let phase = namespace
            .status
            .as_ref()
            .and_then(|status| status.phase.clone());
        Self {
            name: namespace.metadata.name.clone().unwrap_or_default(),
            status: match phase {
                _ if deleting => "Terminating".to_owned(),
                Some(phase) => phase,
                None => "Active".to_owned(),
            },
            created_unix: namespace
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|created| created.0.as_second()),
        }
    }

    fn api(client: Client, _: Option<&str>) -> Api<Namespace> {
        Api::all(client)
    }

    fn namespace(&self) -> &str {
        ""
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// What the details panel shows for one namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceDetails {
    pub labels: Vec<(String, String)>,
    pub annotations: usize,
    /// What must finish before a terminating namespace goes, such as `kubernetes`.
    pub finalizers: Vec<String>,
    /// Why a terminating namespace is stuck, such as `NamespaceContentRemaining`.
    pub conditions: Vec<Condition>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

impl ObjectDetails for NamespaceDetails {
    type Object = Namespace;

    fn api(client: Client, _: &str) -> Api<Namespace> {
        Api::all(client)
    }

    fn read(
        mut namespace: Namespace,
        events: Result<Vec<ObjectEvent>, String>,
    ) -> Result<Self, String> {
        let yaml = to_yaml(&mut namespace)?;
        let metadata = &namespace.metadata;
        Ok(Self {
            labels: metadata
                .labels
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            annotations: metadata.annotations.as_ref().map_or(0, |a| a.len()),
            finalizers: namespace
                .spec
                .iter()
                .flat_map(|spec| spec.finalizers.iter().flatten())
                .cloned()
                .collect(),
            conditions: namespace
                .status
                .iter()
                .flat_map(|status| status.conditions.iter().flatten())
                .map(|condition| Condition {
                    kind: condition.type_.clone(),
                    status: condition.status.clone(),
                    reason: condition.reason.clone().unwrap_or_default(),
                    message: condition.message.clone().unwrap_or_default(),
                })
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

    fn namespace(metadata: Value, status: Value) -> Namespace {
        let mut object = json!({
            "metadata": {
                "name": "shop",
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "labels": { "team": "payments" },
                "annotations": { "a": "1", "b": "2" },
                "managedFields": [{ "manager": "kubectl" }],
            },
            "spec": { "finalizers": ["kubernetes"] },
            "status": status,
        });
        for (key, value) in metadata.as_object().unwrap() {
            object["metadata"][key] = value.clone();
        }
        serde_json::from_value(object).unwrap()
    }

    #[test]
    fn row_reads_the_phase_and_age() {
        let row = NamespaceRow::from_object(&namespace(json!({}), json!({ "phase": "Active" })));
        assert_eq!(row.name, "shop");
        assert_eq!(row.namespace(), "");
        assert_eq!(row.status, "Active");
        assert_eq!(row.created_unix, Some(1_767_225_600));
    }

    #[test]
    fn a_namespace_being_deleted_is_terminating() {
        let deleting = json!({ "deletionTimestamp": "2026-01-02T00:00:00Z" });
        let row = NamespaceRow::from_object(&namespace(deleting, json!({ "phase": "Active" })));
        assert_eq!(row.status, "Terminating");
        let row =
            NamespaceRow::from_object(&namespace(json!({}), json!({ "phase": "Terminating" })));
        assert_eq!(row.status, "Terminating");
    }

    #[test]
    fn details_count_annotations_and_list_finalizers() {
        let status = json!({
            "phase": "Terminating",
            "conditions": [{
                "type": "NamespaceContentRemaining", "status": "True",
                "reason": "SomeResourcesRemain", "message": "Some resources are remaining: pods. has 3 resource instances",
            }],
        });
        let details = NamespaceDetails::read(namespace(json!({}), status), Ok(vec![])).unwrap();

        assert_eq!(details.labels, [("team".to_owned(), "payments".to_owned())]);
        assert_eq!(details.annotations, 2);
        assert_eq!(details.finalizers, ["kubernetes"]);
        assert_eq!(details.conditions[0].reason, "SomeResourcesRemain");
        assert!(details.yaml.contains("name: shop"), "{}", details.yaml);
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"}}]}"#;
    const NAMESPACE_LIST: &str = r#"{"kind":"NamespaceList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"shop","resourceVersion":"1"},"status":{"phase":"Active"}}]}"#;

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

    fn serve(namespaces: Reply) -> fake_api::Handler {
        let namespaces = Mutex::new(namespaces);
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
                "/api/v1/namespaces" => Some(namespaces.lock().unwrap().clone()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn namespaces_are_watched_across_the_cluster() {
        let connection = fake_api::connect_with(serve(Reply::Json(NAMESPACE_LIST.into()))).await;
        let mut namespaces = start::<NamespaceRow>(connection.client, Some("web".into()));

        let snapshot = settle(&mut namespaces).await;

        assert_eq!(snapshot.rows[0].name, "shop");
        assert_eq!(snapshot.rows[0].status, "Active");
    }

    #[tokio::test]
    async fn a_refused_namespace_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "namespaces is forbidden: User \\\"jane\\\" cannot list resource \\\"namespaces\\\" in API group \\\"\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut namespaces = start::<NamespaceRow>(connection.client, None);

        let refused = settle(&mut namespaces).await;
        let listed = settle(&mut pods).await;

        assert!(
            matches!(
                &refused.error,
                Some(WatchError::Forbidden {
                    namespace: None,
                    ..
                })
            ),
            "{:?}",
            refused.error
        );
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "api-1");
    }
}
