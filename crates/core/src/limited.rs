//! What a user with limited permissions can read, checked against a simulated cluster with
//! RBAC on. `scripts/sim-cluster.sh limited` makes the user, and `check-limited` runs this.

use std::env;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::Node;
use kube::Api;
use kube::api::ListParams;

use crate::cluster::{ClusterError, Contexts};
use crate::deployments::DeploymentRow;
use crate::details::{self, PodDetails};
use crate::follow;
use crate::logs::{self, LogTarget};
use crate::pods::PodRow;
use crate::watch::{self, ResourceWatch, Row, Snapshot, WatchError};

const WAIT: Duration = Duration::from_secs(30);

/// Waits for a snapshot that has listed or failed.
async fn settle<R: Row>(watch: &mut ResourceWatch<R>) -> Arc<Snapshot<R>> {
    tokio::time::timeout(WAIT, async {
        loop {
            let snapshot = watch.next().await.expect("watch ended");
            if snapshot.synced || snapshot.error.is_some() {
                return snapshot;
            }
        }
    })
    .await
    .expect("the watch never listed")
}

/// The context alone, so nothing else in the kubeconfig can be reached.
fn only_context(context: &str) -> Contexts {
    let output = Command::new("kubectl")
        .args(["config", "view", "--raw", "--minify", "--flatten"])
        .arg(format!("--context={context}"))
        .output()
        .expect("kubectl runs");
    assert!(output.status.success(), "no context {context}");
    Contexts::from_yaml(&String::from_utf8(output.stdout).unwrap()).unwrap()
}

fn forbidden(error: &Option<WatchError>) -> (Option<String>, String) {
    match error {
        Some(WatchError::Forbidden { namespace, message }) => (namespace.clone(), message.clone()),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the limited user of a simulated cluster"]
async fn limited_user() {
    let context = env::var("TAFFRAIL_LIMITED_CONTEXT").expect("set TAFFRAIL_LIMITED_CONTEXT");
    assert!(context.starts_with("kwok-"), "only a simulated cluster");
    let namespace = env::var("TAFFRAIL_LIMITED_NAMESPACE").unwrap_or_else(|_| "team-a".into());
    let connection = only_context(&context)
        .connect(context.clone())
        .await
        .unwrap();
    let client = connection.client.clone();
    assert_eq!(connection.namespace, namespace);

    let mut pods = watch::start::<PodRow>(client.clone(), None);
    let refused = settle(&mut pods).await;
    let (scope, message) = forbidden(&refused.error);
    println!("Pods, all namespaces: {message}");
    assert_eq!(scope, None);
    assert!(
        message.contains(r#"cannot list resource "pods""#),
        "{message}"
    );

    pods.set_namespace(Some(namespace.clone()));
    let listed = settle(&mut pods).await;
    assert_eq!(listed.error, None);
    assert!(!listed.rows.is_empty(), "no pods in {namespace}");
    assert!(listed.rows.iter().all(|row| row.namespace == namespace));
    println!("Pods in {namespace}: {} listed", listed.rows.len());

    let mut deployments = watch::start::<DeploymentRow>(client.clone(), Some(namespace.clone()));
    let refused = loop {
        let snapshot = settle(&mut deployments).await;
        if forbidden(&snapshot.error).0.is_some() {
            break snapshot;
        }
    };
    let (scope, message) = forbidden(&refused.error);
    println!("Deployments in {namespace}: {message}");
    assert_eq!(scope.as_deref(), Some(namespace.as_str()));
    assert!(
        message.contains(r#"cannot list resource "deployments""#),
        "{message}"
    );

    let nodes = Api::<Node>::all(client.clone())
        .list(&ListParams::default())
        .await;
    let Err(kube::Error::Api(status)) = nodes else {
        panic!("expected the node list to be refused, got {nodes:?}");
    };
    println!("Nodes: {}", status.message);
    assert_eq!(status.code, 403);
    assert!(status.message.contains(r#"cannot list resource "nodes""#));

    let pod = listed.rows[0].clone();
    let pod_details =
        details::load::<PodDetails>(client.clone(), namespace.clone(), pod.name.clone())
            .await
            .unwrap();
    let Err(events) = &pod_details.events else {
        panic!("expected events to be refused");
    };
    println!("Events of {}: {events}", pod.name);
    assert!(
        events.contains(r#"cannot list resource "events""#),
        "{events}"
    );
    assert!(!pod_details.yaml.is_empty());

    let workload = follow::resolve(client.clone(), pod.clone()).await;
    let Err(ClusterError::ReadWorkload { workload, error }) = workload else {
        panic!("expected the workload read to be refused, got {workload:?}");
    };
    println!("Workload of {}: {workload}: {error}", pod.name);
    assert!(error.contains("forbidden"), "{error}");

    let targets: Vec<LogTarget> = listed
        .rows
        .iter()
        .take(3)
        .map(|row| LogTarget {
            namespace: row.namespace.clone(),
            pod: row.name.clone(),
            container: row.containers[0].clone(),
        })
        .collect();
    let mut tail = logs::start(client, targets).unwrap();
    let reader = tail.reader();
    let lines = tokio::time::timeout(WAIT, async {
        loop {
            let progress = tail.next().await.expect("tail ended");
            let lines = reader.read(0..progress.end).unwrap();
            if lines.iter().any(|line| line.gap.is_none()) {
                return lines;
            }
        }
    })
    .await
    .expect("no log lines arrived");
    println!("Logs: {} lines from 3 pods", lines.len());
}
