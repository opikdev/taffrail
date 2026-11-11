use k8s_openapi::api::batch::v1::{Job, JobCondition, JobSpec, JobStatus};
use kube::{Api, Client};

use crate::deployments::Condition;
use crate::details::{ObjectDetails, ObjectEvent, to_yaml};
use crate::follow::{Follow, controller};
use crate::selector::Selector;
use crate::watch::{Row, namespaced_api};
use crate::workload::{created_unix, describe_selector, images, labels};

/// The fields of a Job that its table row and actions use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobRow {
    pub namespace: String,
    pub name: String,
    /// Where it stands: `Complete`, `Running`, `Suspended`, or why it failed, such as
    /// `BackoffLimitExceeded`.
    pub status: String,
    pub succeeded: i32,
    /// The successful pods it needs, or `None` when any one success completes it.
    pub completions: Option<i32>,
    pub parallelism: i32,
    pub start_unix: Option<i64>,
    /// When it completed, or when it failed.
    pub completion_unix: Option<i64>,
    pub created_unix: Option<i64>,
    /// The CronJob that created it, if one did.
    pub cron_job: Option<String>,
    /// `None` if it has no selector Taffrail can read.
    pub selector: Option<Selector>,
}

impl JobRow {
    /// Follows this Job's pods, so a retried pod joins a tail.
    pub fn follow(&self) -> Option<Follow> {
        Some(Follow {
            label: format!("job/{}", self.name),
            namespace: Some(self.namespace.clone()),
            selector: self.selector.clone()?,
        })
    }

    /// How long it has run, or ran, at `now` in Unix seconds.
    pub fn duration(&self, now: i64) -> Option<i64> {
        let start = self.start_unix?;
        Some(self.completion_unix.unwrap_or(now) - start)
    }
}

fn condition<'a>(status: &'a JobStatus, kind: &str) -> Option<&'a JobCondition> {
    status
        .conditions
        .iter()
        .flatten()
        .find(|condition| condition.type_ == kind && condition.status == "True")
}

/// Where a Job stands, from its conditions and pod counts. A failure names its reason, as
/// a pod's status does.
fn job_status(spec: &JobSpec, status: &JobStatus) -> String {
    let failed = condition(status, "Failed").or_else(|| condition(status, "FailureTarget"));
    if let Some(failed) = failed {
        return failed
            .reason
            .clone()
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| "Failed".to_owned());
    }
    if condition(status, "Complete").is_some() || condition(status, "SuccessCriteriaMet").is_some()
    {
        return "Complete".to_owned();
    }
    let word = if spec.suspend == Some(true) {
        "Suspended"
    } else if status.failed.unwrap_or_default() > 0 {
        "Retrying"
    } else if status.active.unwrap_or_default() > 0 {
        "Running"
    } else {
        "Pending"
    };
    word.to_owned()
}

fn time(time: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>) -> Option<i64> {
    time.map(|time| time.0.as_second())
}

fn cron_job(job: &Job) -> Option<String> {
    controller(&job.metadata)
        .filter(|(kind, _)| *kind == "CronJob")
        .map(|(_, name)| name.to_owned())
}

impl Row for JobRow {
    type Object = Job;

    fn from_object(job: &Job) -> Self {
        let spec = job.spec.clone().unwrap_or_default();
        let status = job.status.clone().unwrap_or_default();
        Self {
            namespace: job.metadata.namespace.clone().unwrap_or_default(),
            name: job.metadata.name.clone().unwrap_or_default(),
            status: job_status(&spec, &status),
            succeeded: status.succeeded.unwrap_or_default(),
            completions: spec.completions,
            parallelism: spec.parallelism.unwrap_or(1),
            start_unix: time(status.start_time.as_ref()),
            completion_unix: time(status.completion_time.as_ref()).or_else(|| {
                let failed = condition(&status, "Failed")?;
                time(failed.last_transition_time.as_ref())
            }),
            created_unix: created_unix(&job.metadata),
            cron_job: cron_job(job),
            selector: spec
                .selector
                .as_ref()
                .and_then(|selector| Selector::from_label_selector(selector).ok()),
        }
    }

    fn api(client: Client, namespace: Option<&str>) -> Api<Job> {
        namespaced_api(client, namespace)
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// What the details panel shows for one Job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobDetails {
    pub active: i32,
    pub ready: i32,
    pub failed: i32,
    /// Failed pods it retries before it fails.
    pub backoff_limit: i32,
    /// `NonIndexed` or `Indexed`.
    pub completion_mode: String,
    /// Seconds it may run before it fails, if limited.
    pub active_deadline: Option<i64>,
    /// Seconds it is kept after it finishes, if limited.
    pub ttl_after_finished: Option<i32>,
    pub selector: String,
    pub conditions: Vec<Condition>,
    pub images: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub yaml: String,
    pub events: Result<Vec<ObjectEvent>, String>,
}

impl ObjectDetails for JobDetails {
    type Object = Job;

    fn api(client: Client, namespace: &str) -> Api<Job> {
        Api::namespaced(client, namespace)
    }

    fn read(mut job: Job, events: Result<Vec<ObjectEvent>, String>) -> Result<Self, String> {
        let yaml = to_yaml(&mut job)?;
        let spec = job.spec.clone().unwrap_or_default();
        let status = job.status.clone().unwrap_or_default();
        let conditions = status
            .conditions
            .iter()
            .flatten()
            .map(|condition| Condition {
                kind: condition.type_.clone(),
                status: condition.status.clone(),
                reason: condition.reason.clone().unwrap_or_default(),
                message: condition.message.clone().unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            active: status.active.unwrap_or_default(),
            ready: status.ready.unwrap_or_default(),
            failed: status.failed.unwrap_or_default(),
            backoff_limit: spec.backoff_limit.unwrap_or(6),
            completion_mode: spec
                .completion_mode
                .clone()
                .unwrap_or_else(|| "NonIndexed".to_owned()),
            active_deadline: spec.active_deadline_seconds,
            ttl_after_finished: spec.ttl_seconds_after_finished,
            selector: spec
                .selector
                .as_ref()
                .map_or_else(|| "none".to_owned(), describe_selector),
            conditions,
            images: images(&spec.template),
            labels: labels(&job.metadata),
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

    fn job(spec: Value, status: Value) -> Job {
        serde_json::from_value(json!({
            "metadata": {
                "namespace": "shop",
                "name": "nightly-report-29380",
                "creationTimestamp": "2026-01-01T00:00:00Z",
                "ownerReferences": [{
                    "apiVersion": "batch/v1", "kind": "CronJob", "name": "nightly-report",
                    "uid": "c", "controller": true,
                }],
                "managedFields": [{ "manager": "kube-controller-manager" }],
            },
            "spec": spec,
            "status": status,
        }))
        .unwrap()
    }

    fn spec(completions: i32) -> Value {
        json!({
            "completions": completions,
            "parallelism": 2,
            "selector": { "matchLabels": { "batch.kubernetes.io/controller-uid": "u1" } },
            "template": { "spec": { "containers": [{ "name": "report", "image": "report:7" }] } },
        })
    }

    fn running() -> Value {
        json!({ "startTime": "2026-01-01T00:00:10Z", "active": 2, "succeeded": 1 })
    }

    fn status_of(spec: Value, status: Value) -> String {
        JobRow::from_object(&job(spec, status)).status
    }

    #[test]
    fn row_counts_completions_and_names_its_cron_job() {
        let row = JobRow::from_object(&job(spec(3), running()));

        assert_eq!(row.status, "Running");
        assert_eq!(
            (row.succeeded, row.completions, row.parallelism),
            (1, Some(3), 2)
        );
        assert_eq!(row.cron_job.as_deref(), Some("nightly-report"));
        assert_eq!(row.start_unix, Some(1_767_225_610));
        assert_eq!(row.duration(1_767_225_670), Some(60));
        let follow = row.follow().unwrap();
        assert_eq!(follow.label, "job/nightly-report-29380");
        assert_eq!(
            follow.selector.to_string(),
            "batch.kubernetes.io/controller-uid=u1"
        );
    }

    #[test]
    fn a_complete_job_stops_its_duration() {
        let status = json!({
            "startTime": "2026-01-01T00:00:10Z",
            "completionTime": "2026-01-01T00:02:10Z",
            "succeeded": 3,
            "conditions": [
                { "type": "SuccessCriteriaMet", "status": "True" },
                { "type": "Complete", "status": "True" },
            ],
        });
        let row = JobRow::from_object(&job(spec(3), status));
        assert_eq!(row.status, "Complete");
        assert_eq!(row.duration(i64::MAX), Some(120));
    }

    #[test]
    fn a_failed_job_names_its_reason_and_stops_its_duration() {
        let status = json!({
            "startTime": "2026-01-01T00:00:10Z",
            "failed": 7,
            "conditions": [
                { "type": "FailureTarget", "status": "True", "reason": "BackoffLimitExceeded" },
                {
                    "type": "Failed", "status": "True", "reason": "BackoffLimitExceeded",
                    "lastTransitionTime": "2026-01-01T00:01:10Z",
                },
            ],
        });
        let row = JobRow::from_object(&job(spec(1), status));
        assert_eq!(row.status, "BackoffLimitExceeded");
        assert_eq!(row.duration(i64::MAX), Some(60));

        let status = json!({ "conditions": [{ "type": "Failed", "status": "True" }] });
        assert_eq!(status_of(spec(1), status), "Failed");
    }

    #[test]
    fn suspended_retrying_and_pending_jobs_say_so() {
        let mut suspended = spec(1);
        suspended["suspend"] = json!(true);
        assert_eq!(status_of(suspended, json!({})), "Suspended");
        assert_eq!(
            status_of(spec(1), json!({ "active": 1, "failed": 2 })),
            "Retrying"
        );
        assert_eq!(status_of(spec(1), json!({})), "Pending");
    }

    #[test]
    fn a_job_without_a_cron_job_names_none() {
        let mut job = job(spec(1), running());
        job.metadata.owner_references = None;
        assert_eq!(JobRow::from_object(&job).cron_job, None);
    }

    #[test]
    fn details_describe_limits_counts_and_conditions() {
        let mut spec = spec(3);
        spec["backoffLimit"] = json!(2);
        spec["activeDeadlineSeconds"] = json!(600);
        spec["ttlSecondsAfterFinished"] = json!(3600);
        spec["completionMode"] = json!("Indexed");
        let status = json!({
            "active": 1, "ready": 1, "failed": 1,
            "conditions": [{ "type": "Suspended", "status": "False", "reason": "JobResumed" }],
        });
        let details = JobDetails::read(job(spec, status), Ok(vec![])).unwrap();

        assert_eq!((details.active, details.ready, details.failed), (1, 1, 1));
        assert_eq!(details.backoff_limit, 2);
        assert_eq!(details.completion_mode, "Indexed");
        assert_eq!(details.active_deadline, Some(600));
        assert_eq!(details.ttl_after_finished, Some(3600));
        assert_eq!(details.selector, "batch.kubernetes.io/controller-uid=u1");
        assert_eq!(
            details.images,
            [("report".to_owned(), "report:7".to_owned())]
        );
        assert_eq!(details.conditions[0].reason, "JobResumed");
        assert!(details.yaml.contains("name: nightly-report-29380"));
        assert!(!details.yaml.contains("managedFields"), "{}", details.yaml);
    }

    const POD_LIST: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"report-x2k","resourceVersion":"1"}}]}"#;
    const JOB_LIST: &str = r#"{"kind":"JobList","apiVersion":"batch/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"report","resourceVersion":"1"},"spec":{"completions":1,"template":{}},"status":{"active":1}}]}"#;

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

    fn serve(jobs: Reply) -> fake_api::Handler {
        let jobs = Mutex::new(jobs);
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
                "/apis/batch/v1/jobs" => Some(jobs.lock().unwrap().clone()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn jobs_are_watched_like_pods() {
        let connection = fake_api::connect_with(serve(Reply::Json(JOB_LIST.into()))).await;
        let mut jobs = start::<JobRow>(connection.client, None);

        let snapshot = settle(&mut jobs).await;

        assert_eq!(snapshot.rows[0].name, "report");
        assert_eq!(snapshot.rows[0].status, "Running");
    }

    #[tokio::test]
    async fn a_refused_job_list_leaves_the_pod_watch_working() {
        let connection = fake_api::connect_with(serve(Reply::Forbidden(
            "jobs.batch is forbidden: User \\\"jane\\\" cannot list resource \\\"jobs\\\" in API group \\\"batch\\\" at the cluster scope",
        )))
        .await;
        let mut pods = start::<PodRow>(connection.client.clone(), None);
        let mut jobs = start::<JobRow>(connection.client, None);

        let refused = settle(&mut jobs).await;
        let listed = settle(&mut pods).await;

        let Some(WatchError::Forbidden { namespace, message }) = &refused.error else {
            panic!("expected a refusal, got {:?}", refused.error);
        };
        assert_eq!(*namespace, None);
        assert!(message.contains("cannot list resource"), "{message}");
        assert_eq!(listed.error, None);
        assert_eq!(listed.rows[0].name, "report-x2k");
    }
}
