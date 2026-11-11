use std::collections::HashSet;
use std::sync::Arc;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;

use crate::cluster::ClusterError;
use crate::logs::{LogTarget, MAX_TAIL_PODS};
use crate::pods::{PodKey, PodRow, PodSnapshot};
use crate::reason::reason;
use crate::selector::Selector;
use crate::watch::Row;

/// Pods chosen by a selector rather than by name, so that pods which appear later join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Follow {
    /// What the log dock names, such as `deployment/checkout-api`.
    pub label: String,
    /// `None` follows matching pods in every namespace.
    pub namespace: Option<String>,
    pub selector: Selector,
}

/// One container a follow wants, and whether to read its log from the first line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub target: LogTarget,
    pub uid: String,
    pub from_start: bool,
}

/// The pods a follow has chosen from one snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub members: Vec<Member>,
    pub pods: usize,
    pub matching: usize,
}

impl Follow {
    /// Follows pods that match `selector`, in `namespace` or in every namespace.
    pub fn selector(namespace: Option<String>, selector: Selector) -> Self {
        let label = match &namespace {
            Some(namespace) => format!("{selector} in {namespace}"),
            None => selector.to_string(),
        };
        Self {
            label,
            namespace,
            selector,
        }
    }

    pub fn matches(&self, row: &PodRow) -> bool {
        self.namespace
            .as_ref()
            .is_none_or(|namespace| *namespace == row.namespace)
            && self.selector.matches(&row.labels)
    }

    /// Chooses up to `MAX_TAIL_PODS` matching pods. Pods already followed stay, so a
    /// rollout does not restart their streams, and the newest pods fill the free places.
    /// Pods created at or after `started_unix` are read from their first line.
    pub(crate) fn choose(
        &self,
        snapshot: &PodSnapshot,
        current: &HashSet<PodKey>,
        started_unix: i64,
    ) -> Chosen {
        let mut matching: Vec<&Arc<PodRow>> = snapshot
            .rows
            .iter()
            .filter(|row| self.matches(row))
            .collect();
        let count = matching.len();
        matching.sort_by_cached_key(|row| {
            (
                !current.contains(&row.key()),
                std::cmp::Reverse(row.created_unix),
            )
        });
        matching.truncate(MAX_TAIL_PODS);
        let members = matching
            .iter()
            .flat_map(|row| {
                let from_start = row
                    .created_unix
                    .is_some_and(|created| created >= started_unix);
                row.containers.iter().map(move |container| Member {
                    target: LogTarget {
                        namespace: row.namespace.clone(),
                        pod: row.name.clone(),
                        container: container.clone(),
                    },
                    uid: row.uid.clone(),
                    from_start,
                })
            })
            .collect();
        Chosen {
            members,
            pods: matching.len(),
            matching: count,
        }
    }
}

fn workload_label(kind: &str, name: &str) -> String {
    format!("{}/{name}", kind.to_lowercase())
}

async fn read<K>(client: &Client, namespace: &str, name: &str) -> Result<K, ClusterError>
where
    K: Resource<Scope = k8s_openapi::NamespaceResourceScope, DynamicType = ()>
        + Clone
        + DeserializeOwned
        + std::fmt::Debug,
{
    Api::<K>::namespaced(client.clone(), namespace)
        .get(name)
        .await
        .map_err(|error| ClusterError::ReadWorkload {
            workload: workload_label(&K::kind(&()), name),
            error: reason(&error),
        })
}

/// The kind and name of the object that controls this one, such as a pod's ReplicaSet.
pub(crate) fn controller(meta: &ObjectMeta) -> Option<(&str, &str)> {
    meta.owner_references
        .iter()
        .flatten()
        .find(|owner| owner.controller == Some(true))
        .map(|owner| (owner.kind.as_str(), owner.name.as_str()))
}

fn follow_workload(
    kind: &str,
    namespace: &str,
    name: &str,
    selector: Option<&LabelSelector>,
) -> Result<Follow, ClusterError> {
    let label = workload_label(kind, name);
    let selector = selector
        .ok_or_else(|| "It has no selector".to_owned())
        .and_then(Selector::from_label_selector)
        .map_err(|error| ClusterError::FollowWorkload {
            workload: label.clone(),
            error,
        })?;
    Ok(Follow {
        label,
        namespace: Some(namespace.to_owned()),
        selector,
    })
}

/// Follows the workload that controls `pod`. A ReplicaSet that a Deployment controls
/// resolves to the Deployment, so a rollout's new ReplicaSet joins too.
pub(crate) async fn resolve(client: Client, pod: Arc<PodRow>) -> Result<Follow, ClusterError> {
    let namespace = pod.namespace.as_str();
    let Some(owner) = &pod.owner else {
        return Err(ClusterError::NoWorkload {
            pod: pod.name.clone(),
        });
    };
    let name = owner.name.as_str();
    match owner.kind.as_str() {
        "ReplicaSet" => {
            let set: ReplicaSet = read(&client, namespace, name).await?;
            if let Some(("Deployment", deployment)) = controller(&set.metadata) {
                let deployment: Deployment = read(&client, namespace, deployment).await?;
                let selector = Some(&deployment.spec.selector);
                return follow_workload(
                    "Deployment",
                    namespace,
                    deployment_name(&deployment),
                    selector,
                );
            }
            let selector = Some(&set.spec.selector);
            follow_workload("ReplicaSet", namespace, name, selector)
        }
        "StatefulSet" => {
            let set: StatefulSet = read(&client, namespace, name).await?;
            let selector = Some(&set.spec.selector);
            follow_workload("StatefulSet", namespace, name, selector)
        }
        "DaemonSet" => {
            let set: DaemonSet = read(&client, namespace, name).await?;
            let selector = Some(&set.spec.selector);
            follow_workload("DaemonSet", namespace, name, selector)
        }
        "Job" => {
            let job: Job = read(&client, namespace, name).await?;
            let selector = job.spec.as_ref().and_then(|spec| spec.selector.as_ref());
            follow_workload("Job", namespace, name, selector)
        }
        kind => Err(ClusterError::UnsupportedWorkload {
            workload: workload_label(kind, name),
        }),
    }
}

fn deployment_name(deployment: &Deployment) -> &str {
    deployment.metadata.name.as_deref().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::fake_api::{self, Reply, Request, Route};
    use crate::pods::Owner;

    fn pod(namespace: &str, name: &str, app: &str, created: i64) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: namespace.into(),
            name: name.into(),
            uid: format!("uid-{name}"),
            created_unix: Some(created),
            containers: vec!["app".into()],
            labels: BTreeMap::from([("app".into(), app.into())]),
            ..PodRow::default()
        })
    }

    fn snapshot(rows: Vec<Arc<PodRow>>) -> PodSnapshot {
        PodSnapshot {
            rows,
            synced: true,
            error: None,
        }
    }

    fn follow_app(app: &str, namespace: Option<&str>) -> Follow {
        Follow::selector(
            namespace.map(str::to_owned),
            Selector::parse(&format!("app={app}")).unwrap(),
        )
    }

    fn pods(chosen: &Chosen) -> Vec<&str> {
        chosen
            .members
            .iter()
            .map(|member| member.target.pod.as_str())
            .collect()
    }

    #[test]
    fn a_selector_follow_names_its_namespace() {
        assert_eq!(follow_app("api", Some("shop")).label, "app=api in shop");
        assert_eq!(follow_app("api", None).label, "app=api");
    }

    #[test]
    fn chooses_matching_pods_in_the_namespace() {
        let rows = vec![
            pod("shop", "api-1", "api", 10),
            pod("shop", "web-1", "web", 10),
            pod("test", "api-2", "api", 10),
        ];

        let chosen = follow_app("api", Some("shop")).choose(&snapshot(rows), &HashSet::new(), 0);

        assert_eq!(pods(&chosen), ["api-1"]);
        assert_eq!((chosen.pods, chosen.matching), (1, 1));
    }

    #[test]
    fn every_container_of_a_chosen_pod_is_a_member() {
        let mut row = (*pod("shop", "api-1", "api", 10)).clone();
        row.containers = vec!["app".into(), "proxy".into()];

        let chosen =
            follow_app("api", None).choose(&snapshot(vec![Arc::new(row)]), &HashSet::new(), 0);

        let containers: Vec<_> = chosen
            .members
            .iter()
            .map(|member| member.target.container.as_str())
            .collect();
        assert_eq!(containers, ["app", "proxy"]);
        assert_eq!(chosen.pods, 1);
    }

    #[test]
    fn the_limit_keeps_current_pods_and_fills_with_the_newest() {
        let rows: Vec<_> = (0..MAX_TAIL_PODS + 10)
            .map(|ix| pod("shop", &format!("api-{ix:03}"), "api", ix as i64))
            .collect();
        let current = HashSet::from([("shop".to_owned(), "api-000".to_owned())]);

        let chosen = follow_app("api", None).choose(&snapshot(rows), &current, 0);

        assert_eq!(chosen.pods, MAX_TAIL_PODS);
        assert_eq!(chosen.matching, MAX_TAIL_PODS + 10);
        let chosen = pods(&chosen);
        assert_eq!(chosen[0], "api-000");
        assert_eq!(chosen[1], format!("api-{:03}", MAX_TAIL_PODS + 9));
        assert!(!chosen.contains(&"api-001"));
    }

    #[test]
    fn pods_created_after_the_follow_started_are_read_from_the_start() {
        let rows = vec![
            pod("shop", "old", "api", 99),
            pod("shop", "new", "api", 100),
        ];

        let chosen = follow_app("api", None).choose(&snapshot(rows), &HashSet::new(), 100);

        let from_start: Vec<_> = chosen
            .members
            .iter()
            .map(|member| (member.target.pod.as_str(), member.from_start))
            .collect();
        assert_eq!(from_start, [("new", true), ("old", false)]);
    }

    fn owned_by(kind: &str, name: &str) -> Arc<PodRow> {
        let mut row = (*pod("shop", "api-7f9c-x2k", "api", 10)).clone();
        row.owner = Some(Owner {
            kind: kind.into(),
            name: name.into(),
        });
        Arc::new(row)
    }

    const REPLICA_SET: &str = r#"{"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{"name":"api-7f9c","namespace":"shop","ownerReferences":[{"apiVersion":"apps/v1","kind":"Deployment","name":"api","uid":"d","controller":true}]},"spec":{"selector":{"matchLabels":{"app":"api","pod-template-hash":"7f9c"}}}}"#;
    const DEPLOYMENT: &str = r#"{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"api","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"api"}},"template":{}}}"#;
    const BARE_REPLICA_SET: &str = r#"{"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{"name":"api-7f9c","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"api","pod-template-hash":"7f9c"}}}}"#;
    const STATEFUL_SET: &str = r#"{"apiVersion":"apps/v1","kind":"StatefulSet","metadata":{"name":"db","namespace":"shop"},"spec":{"selector":{"matchExpressions":[{"key":"app","operator":"In","values":["db"]}]},"template":{},"serviceName":"db"}}"#;
    const JOB: &str = r#"{"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"migrate","namespace":"shop"},"spec":{"selector":{"matchLabels":{"batch.kubernetes.io/controller-uid":"j1"}},"template":{}}}"#;

    async fn resolve_with(routes: Vec<Route>, pod: Arc<PodRow>) -> Result<Follow, ClusterError> {
        let connection = fake_api::connect(routes).await;
        resolve(connection.client, pod).await
    }

    #[tokio::test]
    async fn a_replica_set_resolves_to_its_deployment() {
        let routes = vec![
            (
                "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c",
                REPLICA_SET,
            ),
            ("/apis/apps/v1/namespaces/shop/deployments/api", DEPLOYMENT),
        ];

        let follow = resolve_with(routes, owned_by("ReplicaSet", "api-7f9c"))
            .await
            .unwrap();

        assert_eq!(follow.label, "deployment/api");
        assert_eq!(follow.namespace.as_deref(), Some("shop"));
        assert_eq!(follow.selector.to_string(), "app=api");
    }

    #[tokio::test]
    async fn a_replica_set_without_a_deployment_is_followed_itself() {
        let routes = vec![(
            "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c",
            BARE_REPLICA_SET,
        )];

        let follow = resolve_with(routes, owned_by("ReplicaSet", "api-7f9c"))
            .await
            .unwrap();

        assert_eq!(follow.label, "replicaset/api-7f9c");
        assert_eq!(
            follow.selector.to_string(),
            "app=api,pod-template-hash=7f9c"
        );
    }

    #[tokio::test]
    async fn stateful_sets_and_jobs_resolve_to_their_selectors() {
        let routes = vec![
            (
                "/apis/apps/v1/namespaces/shop/statefulsets/db",
                STATEFUL_SET,
            ),
            ("/apis/batch/v1/namespaces/shop/jobs/migrate", JOB),
        ];
        let connection = fake_api::connect(routes).await;

        let db = resolve(connection.client.clone(), owned_by("StatefulSet", "db"))
            .await
            .unwrap();
        let job = resolve(connection.client, owned_by("Job", "migrate"))
            .await
            .unwrap();

        assert_eq!(db.label, "statefulset/db");
        assert_eq!(db.selector.to_string(), "app in (db)");
        assert_eq!(job.label, "job/migrate");
        assert_eq!(
            job.selector.to_string(),
            "batch.kubernetes.io/controller-uid=j1"
        );
    }

    #[tokio::test]
    async fn a_pod_without_a_controller_has_no_workload() {
        let error = resolve_with(Vec::new(), pod("shop", "debug", "api", 10))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "debug has no workload to follow");
    }

    #[tokio::test]
    async fn an_unknown_controller_is_not_followed() {
        let error = resolve_with(Vec::new(), owned_by("Rollout", "api"))
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Following a rollout/api is not supported"
        );
    }

    #[tokio::test]
    async fn a_refused_workload_says_the_apis_reason_and_what_to_do() {
        let connection = fake_api::connect_with(std::sync::Arc::new(|request: &Request| {
            (request.path == "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c").then_some(
                Reply::Forbidden(
                    r#"replicasets.apps \"api-7f9c\" is forbidden: User \"jane\" cannot get resource \"replicasets\""#,
                ),
            )
        }))
        .await;

        let error = resolve(connection.client, owned_by("ReplicaSet", "api-7f9c"))
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            r#"Can't read replicaset/api-7f9c to find its pods. Press l to tail this pod alone. The API says: replicasets.apps "api-7f9c" is forbidden: User "jane" cannot get resource "replicasets""#
        );
    }

    #[tokio::test]
    async fn a_workload_that_cannot_be_read_says_which() {
        let error = resolve_with(Vec::new(), owned_by("DaemonSet", "agent"))
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(
            message.starts_with("Can't read daemonset/agent to find its pods. "),
            "{message}"
        );
    }
}
