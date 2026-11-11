use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{DaemonSetDetails, DaemonSetRow, Follow};

use crate::details::{Inspect, field};
use crate::table::{
    CellStyle, ColumnDef, TableRow, Tone, age_text, by_age, format_age, unix_now, urgency,
};
use crate::workload::{
    condition_tone, conditions_section, key_values, labels_section, lines_section, mono_text,
    pairs_section, pod_actions,
};

pub fn daemon_set_tone(status: &str) -> Tone {
    match status {
        "Ready" => Tone::Healthy,
        "Misscheduled" | "Rolling out" | "Outdated" | "Scheduling" | "Not ready" => Tone::Degraded,
        "No nodes" => Tone::Neutral,
        _ => Tone::Failed,
    }
}

impl TableRow for DaemonSetRow {
    const SINGULAR: &'static str = "DaemonSet";
    const PLURAL: &'static str = "DaemonSets";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 240.,
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
            width: 120.,
            style: CellStyle::Status,
            text: |row, _| row.status.clone().into(),
            compare: |a, b| {
                urgency(a)
                    .cmp(&urgency(b))
                    .then_with(|| a.status.cmp(&b.status))
            },
        },
        ColumnDef {
            name: "Desired",
            width: 68.,
            style: CellStyle::Number,
            text: |row, _| row.desired.to_string().into(),
            compare: |a, b| a.desired.cmp(&b.desired),
        },
        ColumnDef {
            name: "Current",
            width: 68.,
            style: CellStyle::Number,
            text: |row, _| row.current.to_string().into(),
            compare: |a, b| a.current.cmp(&b.current),
        },
        ColumnDef {
            name: "Ready",
            width: 60.,
            style: CellStyle::Number,
            text: |row, _| row.ready.to_string().into(),
            compare: |a, b| a.ready.cmp(&b.ready),
        },
        ColumnDef {
            name: "Up-to-date",
            width: 88.,
            style: CellStyle::Number,
            text: |row, _| row.updated.to_string().into(),
            compare: |a, b| a.updated.cmp(&b.updated),
        },
        ColumnDef {
            name: "Available",
            width: 80.,
            style: CellStyle::Number,
            text: |row, _| row.available.to_string().into(),
            compare: |a, b| a.available.cmp(&b.available),
        },
        ColumnDef {
            name: "Node selector",
            width: 180.,
            style: CellStyle::Muted,
            text: |row, _| row.node_selector.clone().into(),
            compare: |a, b| a.node_selector.cmp(&b.node_selector),
        },
        ColumnDef {
            name: "Age",
            width: 56.,
            style: CellStyle::Number,
            text: |row, now| age_text(row.created_unix, now),
            compare: |a, b| by_age(a.created_unix, b.created_unix),
        },
    ];

    fn tone(&self) -> Tone {
        daemon_set_tone(&self.status)
    }

    fn follow(&self) -> Option<Follow> {
        DaemonSetRow::follow(self)
    }
}

impl Inspect for DaemonSetRow {
    type Details = DaemonSetDetails;

    fn summary(&self, details: Option<&DaemonSetDetails>, cx: &App) -> AnyElement {
        let count =
            |label: &'static str, count: i32| field(label, div().child(count.to_string()), cx);
        let mut fields = v_flex()
            .gap_1p5()
            .child(field(
                "Status",
                div()
                    .text_color(self.tone().color(cx))
                    .child(self.status.clone()),
                cx,
            ))
            .child(count("Desired", self.desired))
            .child(count("Current", self.current))
            .child(count("Ready", self.ready))
            .child(count("Up-to-date", self.updated))
            .child(count("Available", self.available));
        if self.misscheduled > 0 {
            fields = fields.child(count("Misscheduled", self.misscheduled));
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
            fields = fields
                .child(field("Strategy", div().child(details.strategy.clone()), cx))
                .child(field(
                    "Selector",
                    mono_text(details.selector.clone(), cx),
                    cx,
                ));
        }
        let conditions = details.map_or(&[][..], |details| &details.conditions);
        let images = details.map_or(&[][..], |details| &details.images);
        let node_selector = details.map_or(&[][..], |details| &details.node_selector);
        let tolerations = details.map_or(&[][..], |details| &details.tolerations);
        let labels = details.map_or(&[][..], |details| &details.labels);
        v_flex()
            .gap_4()
            .child(fields)
            .child(pod_actions(self.selector.is_some()))
            .children(conditions_section(conditions, condition_tone, cx))
            .children(pairs_section("Images", images.iter().cloned(), cx))
            .children(lines_section(
                "Node selector",
                key_values(node_selector),
                cx,
            ))
            .children(lines_section("Tolerations", tolerations.to_vec(), cx))
            .children(labels_section(labels, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{DaemonSetRow, Snapshot};

    use super::daemon_set_tone;
    use crate::table::{ResourceTable, Tone};

    fn row(name: &str, status: &str, ready: i32) -> Arc<DaemonSetRow> {
        Arc::new(DaemonSetRow {
            namespace: "kube-system".into(),
            name: name.into(),
            status: status.into(),
            desired: 4,
            ready,
            ..DaemonSetRow::default()
        })
    }

    fn table(rows: Vec<Arc<DaemonSetRow>>) -> ResourceTable<DaemonSetRow> {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn names(table: &ResourceTable<DaemonSetRow>) -> Vec<&str> {
        table.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn unavailable_sets_lead_then_degraded_ones() {
        let table = table(vec![
            row("a", "Ready", 4),
            row("b", "Misscheduled", 4),
            row("c", "Unavailable", 0),
            row("d", "No nodes", 0),
            row("e", "Rolling out", 3),
        ]);
        assert_eq!(names(&table), ["c", "b", "e", "a", "d"]);
    }

    #[test]
    fn ready_sorts_by_ready_pods() {
        let mut table = table(vec![row("a", "Ready", 4), row("b", "Ready", 1)]);
        table.cycle_sort(5);
        assert_eq!(names(&table), ["b", "a"]);
    }

    #[test]
    fn each_status_has_a_tone() {
        assert_eq!(daemon_set_tone("Ready"), Tone::Healthy);
        assert_eq!(daemon_set_tone("Misscheduled"), Tone::Degraded);
        assert_eq!(daemon_set_tone("Scheduling"), Tone::Degraded);
        assert_eq!(daemon_set_tone("No nodes"), Tone::Neutral);
        assert_eq!(daemon_set_tone("Unavailable"), Tone::Failed);
    }
}
