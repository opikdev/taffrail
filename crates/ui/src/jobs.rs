use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Condition, Follow, JobDetails, JobRow, ObjectLink};

use crate::details::{Inspect, field, object_link};
use crate::table::{
    CellStyle, ColumnDef, TableRow, Tone, age_text, by_age, format_age, unix_now, urgency,
};
use crate::workload::{conditions_section, labels_section, mono_text, pairs_section, pod_actions};

pub fn job_tone(status: &str) -> Tone {
    match status {
        "Running" => Tone::Healthy,
        "Retrying" | "Pending" => Tone::Degraded,
        "Complete" | "Suspended" => Tone::Neutral,
        _ => Tone::Failed,
    }
}

/// Successful pods out of those it needs, such as `2/3`, or `0/1 of 4` for a Job that any
/// one of 4 parallel pods completes.
fn completions(row: &JobRow) -> SharedString {
    let succeeded = row.succeeded;
    match row.completions {
        Some(completions) => format!("{succeeded}/{completions}"),
        None if row.parallelism > 1 => format!("{succeeded}/1 of {}", row.parallelism),
        None => format!("{succeeded}/1"),
    }
    .into()
}

/// A run time to the second while short, such as `45s`, `2m10s`, `1h5m` or `10m`.
pub fn format_duration(seconds: i64) -> String {
    let (major, unit, minor, minor_unit) = match seconds.max(0) {
        seconds @ ..60 => return format!("{seconds}s"),
        seconds @ ..3_600 => (seconds / 60, "m", seconds % 60, "s"),
        seconds @ ..86_400 => (seconds / 3_600, "h", seconds % 3_600 / 60, "m"),
        seconds => (seconds / 86_400, "d", seconds % 86_400 / 3_600, "h"),
    };
    match minor {
        0 => format!("{major}{unit}"),
        _ => format!("{major}{unit}{minor}{minor_unit}"),
    }
}

fn duration_text(row: &JobRow, now: i64) -> SharedString {
    row.duration(now)
        .map(format_duration)
        .unwrap_or_default()
        .into()
}

impl TableRow for JobRow {
    const SINGULAR: &'static str = "job";
    const PLURAL: &'static str = "jobs";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 300.,
            style: CellStyle::Text,
            text: |row, _| row.name.clone().into(),
            compare: |a, b| a.name.cmp(&b.name),
        },
        ColumnDef {
            name: "Namespace",
            width: 120.,
            style: CellStyle::Muted,
            text: |row, _| row.namespace.clone().into(),
            compare: |a, b| a.namespace.cmp(&b.namespace),
        },
        ColumnDef {
            name: "Status",
            width: 180.,
            style: CellStyle::Status,
            text: |row, _| row.status.clone().into(),
            compare: |a, b| {
                urgency(a)
                    .cmp(&urgency(b))
                    .then_with(|| a.status.cmp(&b.status))
            },
        },
        ColumnDef {
            name: "Completions",
            width: 100.,
            style: CellStyle::Number,
            text: |row, _| completions(row),
            compare: |a, b| (a.succeeded, a.completions).cmp(&(b.succeeded, b.completions)),
        },
        ColumnDef {
            name: "Duration",
            width: 84.,
            style: CellStyle::Number,
            text: duration_text,
            compare: |a, b| {
                let now = unix_now();
                a.duration(now).cmp(&b.duration(now))
            },
        },
        ColumnDef {
            name: "Age",
            width: 60.,
            style: CellStyle::Number,
            text: |row, now| age_text(row.created_unix, now),
            compare: |a, b| by_age(a.created_unix, b.created_unix),
        },
        ColumnDef {
            name: "CronJob",
            width: 200.,
            style: CellStyle::Muted,
            text: |row, _| row.cron_job.clone().unwrap_or_default().into(),
            compare: |a, b| a.cron_job.cmp(&b.cron_job),
        },
    ];

    fn tone(&self) -> Tone {
        job_tone(&self.status)
    }

    fn follow(&self) -> Option<Follow> {
        JobRow::follow(self)
    }
}

/// Whether a Job condition reads as healthy. `Failed` and `FailureTarget` are bad when
/// true, and a false condition says only that it has not happened.
fn condition_tone(condition: &Condition) -> Tone {
    match (condition.kind.as_str(), condition.status.as_str()) {
        ("Failed" | "FailureTarget", "True") => Tone::Failed,
        ("Complete" | "SuccessCriteriaMet", "True") => Tone::Healthy,
        _ => Tone::Neutral,
    }
}

/// Seconds as a limit, such as `10m0s`.
fn limit(seconds: Option<i64>) -> Option<String> {
    seconds.map(format_duration)
}

impl Inspect for JobRow {
    type Details = JobDetails;

    fn summary(&self, details: Option<&JobDetails>, cx: &App) -> AnyElement {
        let now = unix_now();
        let text = |value: String| div().child(value);
        let mut fields = v_flex()
            .gap_1p5()
            .child(field(
                "Status",
                div()
                    .text_color(self.tone().color(cx))
                    .child(self.status.clone()),
                cx,
            ))
            .child(field("Completions", div().child(completions(self)), cx));
        if let Some(details) = details {
            fields = fields
                .child(field("Active", text(details.active.to_string()), cx))
                .child(field("Ready", text(details.ready.to_string()), cx))
                .child(field(
                    "Failed",
                    text(format!(
                        "{}, backoff limit {}",
                        details.failed, details.backoff_limit
                    )),
                    cx,
                ));
        }
        if let Some(duration) = self.duration(now) {
            fields = fields.child(field("Duration", text(format_duration(duration)), cx));
        }
        if let Some(start) = self.start_unix {
            fields = fields.child(field(
                "Started",
                text(format!("{} ago", format_age(now - start))),
                cx,
            ));
        }
        fields = fields.child(field("Namespace", text(self.namespace.clone()), cx));
        if let Some(created) = self.created_unix {
            fields = fields.child(field("Age", text(format_age(now - created)), cx));
        }
        if let Some(cron_job) = &self.cron_job {
            let link = ObjectLink::new("CronJob", &self.namespace, cron_job);
            let label = link.label();
            fields = fields.child(field(
                "Created by",
                div().child(object_link(&link, label)),
                cx,
            ));
        }
        if let Some(details) = details {
            fields = fields
                .child(field("Parallelism", text(self.parallelism.to_string()), cx))
                .child(field("Mode", text(details.completion_mode.clone()), cx));
            if let Some(deadline) = limit(details.active_deadline) {
                fields = fields.child(field("Deadline", text(deadline), cx));
            }
            if let Some(ttl) = limit(details.ttl_after_finished.map(i64::from)) {
                fields = fields.child(field("Kept for", text(ttl), cx));
            }
            fields = fields.child(field(
                "Selector",
                mono_text(details.selector.clone(), cx),
                cx,
            ));
        }
        let conditions = details.map_or(&[][..], |details| &details.conditions);
        let images = details.map_or(&[][..], |details| &details.images);
        let labels = details.map_or(&[][..], |details| &details.labels);
        v_flex()
            .gap_4()
            .child(fields)
            .child(pod_actions(self.selector.is_some()))
            .children(conditions_section(conditions, condition_tone, cx))
            .children(pairs_section("Images", images.iter().cloned(), cx))
            .children(labels_section(labels, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{Condition, JobRow, Snapshot};

    use super::{completions, condition_tone, format_duration, job_tone};
    use crate::table::{ResourceTable, Tone};

    fn row(name: &str, status: &str, succeeded: i32) -> Arc<JobRow> {
        Arc::new(JobRow {
            namespace: "batch".into(),
            name: name.into(),
            status: status.into(),
            succeeded,
            completions: Some(3),
            parallelism: 1,
            ..JobRow::default()
        })
    }

    fn table(rows: Vec<Arc<JobRow>>) -> ResourceTable<JobRow> {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn names(table: &ResourceTable<JobRow>) -> Vec<&str> {
        table.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn failed_jobs_lead_then_retrying_then_running() {
        let table = table(vec![
            row("a", "Complete", 3),
            row("b", "Running", 1),
            row("c", "BackoffLimitExceeded", 0),
            row("d", "Retrying", 0),
            row("e", "Suspended", 0),
        ]);
        assert_eq!(names(&table), ["c", "d", "b", "a", "e"]);
    }

    #[test]
    fn completions_sort_by_succeeded_pods() {
        let mut table = table(vec![row("a", "Running", 2), row("b", "Running", 1)]);
        table.cycle_sort(3);
        assert_eq!(names(&table), ["b", "a"]);
    }

    #[test]
    fn completions_read_as_kubectl_shows_them() {
        let mut job = JobRow {
            succeeded: 1,
            completions: Some(3),
            parallelism: 2,
            ..JobRow::default()
        };
        assert_eq!(completions(&job), "1/3");
        job.completions = None;
        assert_eq!(completions(&job), "1/1 of 2");
        job.parallelism = 1;
        assert_eq!(completions(&job), "1/1");
    }

    #[test]
    fn durations_keep_seconds_while_short() {
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(130), "2m10s");
        assert_eq!(format_duration(3_900), "1h5m");
        assert_eq!(format_duration(90_000), "1d1h");
        assert_eq!(format_duration(600), "10m");
        assert_eq!(format_duration(86_400), "1d");
    }

    #[test]
    fn each_status_has_a_tone() {
        assert_eq!(job_tone("Running"), Tone::Healthy);
        assert_eq!(job_tone("Retrying"), Tone::Degraded);
        assert_eq!(job_tone("Complete"), Tone::Neutral);
        assert_eq!(job_tone("Suspended"), Tone::Neutral);
        assert_eq!(job_tone("DeadlineExceeded"), Tone::Failed);
    }

    #[test]
    fn a_failed_condition_is_bad_only_when_true() {
        let condition = |kind: &str, status: &str| Condition {
            kind: kind.into(),
            status: status.into(),
            reason: String::new(),
            message: String::new(),
        };
        assert_eq!(condition_tone(&condition("Failed", "True")), Tone::Failed);
        assert_eq!(condition_tone(&condition("Failed", "False")), Tone::Neutral);
        assert_eq!(
            condition_tone(&condition("Complete", "True")),
            Tone::Healthy
        );
    }
}
