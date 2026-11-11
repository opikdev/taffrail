use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Follow, ObjectLink, StatefulSetDetails, StatefulSetRow};

use crate::details::{Inspect, field, object_link};
use crate::table::{
    CellStyle, ColumnDef, TableRow, Tone, age_text, by_age, format_age, unix_now, urgency,
};
use crate::workload::{
    condition_tone, conditions_section, labels_section, mono_text, pairs_section, pod_actions,
};

pub fn stateful_set_tone(status: &str) -> Tone {
    match status {
        "Ready" => Tone::Healthy,
        "Rolling out" | "Scaling" | "Outdated" | "Not ready" => Tone::Degraded,
        "ScaledToZero" => Tone::Neutral,
        _ => Tone::Failed,
    }
}

/// A count out of the replicas the StatefulSet wants, such as `2/3`.
fn of_desired(count: i32, row: &StatefulSetRow) -> SharedString {
    format!("{count}/{}", row.desired).into()
}

impl TableRow for StatefulSetRow {
    const SINGULAR: &'static str = "StatefulSet";
    const PLURAL: &'static str = "StatefulSets";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 320.,
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
            width: 150.,
            style: CellStyle::Status,
            text: |row, _| row.status.clone().into(),
            compare: |a, b| {
                urgency(a)
                    .cmp(&urgency(b))
                    .then_with(|| a.status.cmp(&b.status))
            },
        },
        ColumnDef {
            name: "Ready",
            width: 72.,
            style: CellStyle::Number,
            text: |row, _| of_desired(row.ready, row),
            compare: |a, b| (a.ready, a.desired).cmp(&(b.ready, b.desired)),
        },
        ColumnDef {
            name: "Up-to-date",
            width: 92.,
            style: CellStyle::Number,
            text: |row, _| row.updated.to_string().into(),
            compare: |a, b| a.updated.cmp(&b.updated),
        },
        ColumnDef {
            name: "Age",
            width: 60.,
            style: CellStyle::Number,
            text: |row, now| age_text(row.created_unix, now),
            compare: |a, b| by_age(a.created_unix, b.created_unix),
        },
    ];

    fn tone(&self) -> Tone {
        stateful_set_tone(&self.status)
    }

    fn follow(&self) -> Option<Follow> {
        StatefulSetRow::follow(self)
    }
}

impl Inspect for StatefulSetRow {
    type Details = StatefulSetDetails;

    fn summary(&self, details: Option<&StatefulSetDetails>, cx: &App) -> AnyElement {
        let mut fields = v_flex()
            .gap_1p5()
            .child(field(
                "Status",
                div()
                    .text_color(self.tone().color(cx))
                    .child(self.status.clone()),
                cx,
            ))
            .child(field(
                "Ready",
                div().child(of_desired(self.ready, self)),
                cx,
            ))
            .child(field(
                "Up-to-date",
                div().child(self.updated.to_string()),
                cx,
            ));
        if let Some(details) = details {
            fields = fields.child(field(
                "Current",
                div().child(details.current_replicas.to_string()),
                cx,
            ));
        }
        fields = fields.child(field("Namespace", div().child(self.namespace.clone()), cx));
        if let Some(created) = self.created_unix {
            fields = fields.child(field(
                "Age",
                div().child(format_age(unix_now() - created)),
                cx,
            ));
        }
        if let Some(details) = details {
            if !details.service.is_empty() {
                let service = ObjectLink::new("Service", &self.namespace, &details.service);
                let link = object_link(&service, details.service.clone());
                fields = fields.child(field("Service", div().child(link), cx));
            }
            fields = fields
                .child(field("Strategy", div().child(details.strategy.clone()), cx))
                .child(field(
                    "Pod order",
                    div().child(details.pod_management.clone()),
                    cx,
                ));
            let revision = match (&details.current_revision, &details.update_revision) {
                (current, update) if current == update || update.is_empty() => current.clone(),
                (current, update) if current.is_empty() => update.clone(),
                (current, update) => format!("{current} → {update}"),
            };
            if !revision.is_empty() {
                fields = fields.child(field("Revision", mono_text(revision, cx), cx));
            }
            fields = fields.child(field(
                "Selector",
                mono_text(details.selector.clone(), cx),
                cx,
            ));
        }
        let conditions = details.map_or(&[][..], |details| &details.conditions);
        let images = details.map_or(&[][..], |details| &details.images);
        let claims = details.map_or(&[][..], |details| &details.claims);
        let labels = details.map_or(&[][..], |details| &details.labels);
        let claims = claims.iter().map(|claim| {
            let mut value = claim.storage.clone();
            if !claim.class.is_empty() {
                value = match value.is_empty() {
                    true => claim.class.clone(),
                    false => format!("{value}, {}", claim.class),
                };
            }
            (claim.name.clone(), value)
        });
        v_flex()
            .gap_4()
            .child(fields)
            .child(pod_actions(self.selector.is_some()))
            .children(conditions_section(conditions, condition_tone, cx))
            .children(pairs_section("Images", images.iter().cloned(), cx))
            .children(pairs_section("Volume claims", claims, cx))
            .children(labels_section(labels, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{Snapshot, StatefulSetRow};

    use super::stateful_set_tone;
    use crate::table::{ResourceTable, Tone};

    fn row(name: &str, status: &str, ready: i32) -> Arc<StatefulSetRow> {
        Arc::new(StatefulSetRow {
            namespace: "data".into(),
            name: name.into(),
            status: status.into(),
            desired: 3,
            ready,
            ..StatefulSetRow::default()
        })
    }

    fn table(rows: Vec<Arc<StatefulSetRow>>) -> ResourceTable<StatefulSetRow> {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn names(table: &ResourceTable<StatefulSetRow>) -> Vec<&str> {
        table.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn unavailable_sets_lead_then_degraded_ones() {
        let table = table(vec![
            row("a", "Ready", 3),
            row("b", "Rolling out", 2),
            row("c", "Unavailable", 0),
            row("d", "ScaledToZero", 0),
            row("e", "Not ready", 2),
        ]);
        assert_eq!(names(&table), ["c", "b", "e", "a", "d"]);
    }

    #[test]
    fn ready_sorts_by_ready_replicas() {
        let mut table = table(vec![row("a", "Ready", 3), row("b", "Ready", 1)]);
        table.cycle_sort(3);
        assert_eq!(names(&table), ["b", "a"]);
    }

    #[test]
    fn each_status_has_a_tone() {
        assert_eq!(stateful_set_tone("Ready"), Tone::Healthy);
        assert_eq!(stateful_set_tone("Rolling out"), Tone::Degraded);
        assert_eq!(stateful_set_tone("Outdated"), Tone::Degraded);
        assert_eq!(stateful_set_tone("ScaledToZero"), Tone::Neutral);
        assert_eq!(stateful_set_tone("Unavailable"), Tone::Failed);
    }
}
