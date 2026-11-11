use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Condition, NodeDetails, NodeRow};

use crate::details::{Inspect, field, section_title};
use crate::palette::ShowPods;
use crate::table::{
    CellStyle, ColumnDef, PodCounts, RowMatch, SelectorFilter, TableRow, Tone, age_text, by_age,
    format_age, unix_now, urgency,
};

/// Not ready or unknown is failed. A ready node that is cordoned or under pressure is
/// degraded.
pub fn node_tone(status: &str) -> Tone {
    let (ready, problems) = status.split_once(", ").unwrap_or((status, ""));
    match (ready, problems) {
        ("Ready", "") => Tone::Healthy,
        ("Ready", _) => Tone::Degraded,
        _ => Tone::Failed,
    }
}

impl TableRow for NodeRow {
    const SINGULAR: &'static str = "node";
    const PLURAL: &'static str = "nodes";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 300.,
            style: CellStyle::Text,
            text: |row, _| row.name.clone().into(),
            compare: |a, b| a.name.cmp(&b.name),
        },
        ColumnDef {
            name: "Status",
            width: 220.,
            style: CellStyle::Status,
            text: |row, _| row.status.clone().into(),
            compare: |a, b| {
                urgency(a)
                    .cmp(&urgency(b))
                    .then_with(|| a.status.cmp(&b.status))
            },
        },
        ColumnDef {
            name: "Roles",
            width: 140.,
            style: CellStyle::Muted,
            text: |row, _| row.roles.clone().into(),
            compare: |a, b| a.roles.cmp(&b.roles),
        },
        ColumnDef {
            name: "Version",
            width: 100.,
            style: CellStyle::Muted,
            text: |row, _| row.version.clone().into(),
            compare: |a, b| a.version.cmp(&b.version),
        },
        ColumnDef {
            name: "Pods",
            width: 64.,
            style: CellStyle::PodCount,
            text: |_, _| SharedString::default(),
            compare: |_, _| std::cmp::Ordering::Equal,
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
        node_tone(&self.status)
    }

    fn pods(&self) -> Option<(SelectorFilter, Option<String>)> {
        let filter = SelectorFilter {
            label: format!("node/{}", self.name),
            selector: RowMatch::Field {
                name: "spec.nodeName",
                value: self.name.clone(),
            },
        };
        Some((filter, None))
    }

    fn pod_count(&self, counts: &PodCounts) -> Option<usize> {
        Some(counts.by_node.get(&self.name).copied().unwrap_or(0))
    }
}

/// Whether a node condition reads as healthy. Only `Ready` is good when true.
fn condition_tone(condition: &Condition) -> Tone {
    let good_when_true = condition.kind == "Ready";
    match condition.status.as_str() {
        "True" if good_when_true => Tone::Healthy,
        "True" => Tone::Failed,
        "False" if good_when_true => Tone::Failed,
        "False" => Tone::Healthy,
        _ => Tone::Neutral,
    }
}

fn render_condition(condition: &Condition, cx: &App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    let color = condition_tone(condition).color(cx);
    v_flex()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().flex_1().min_w_0().child(condition.kind.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .child(condition.status.clone()),
                ),
        )
        .when(!condition.message.is_empty(), |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(condition.message.clone()),
            )
        })
}

/// A list of muted monospace lines under a title, left out when empty.
fn mono_section(
    title: &'static str,
    lines: Vec<String>,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    let theme = cx.theme();
    let (mono, muted) = (theme.mono_font_family.clone(), theme.muted_foreground);
    (!lines.is_empty()).then(|| {
        v_flex()
            .gap_1()
            .child(section_title(title, cx))
            .children(lines.into_iter().map(move |line| {
                div()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .child(line)
            }))
    })
}

impl Inspect for NodeRow {
    type Details = NodeDetails;

    fn summary(&self, details: Option<&NodeDetails>, cx: &App) -> AnyElement {
        let mut fields = v_flex().gap_1p5();
        fields = fields.child(field(
            "Status",
            div()
                .text_color(self.tone().color(cx))
                .child(self.status.clone()),
            cx,
        ));
        if !self.roles.is_empty() {
            fields = fields.child(field("Roles", div().child(self.roles.clone()), cx));
        }
        fields = fields.child(field("Kubelet", div().child(self.version.clone()), cx));
        if let Some(created) = self.created_unix {
            fields = fields.child(field(
                "Age",
                div().child(format_age(unix_now() - created)),
                cx,
            ));
        }
        if let Some(details) = details {
            for (label, value) in [
                ("OS image", &details.os_image),
                ("Platform", &details.platform),
                ("Kernel", &details.kernel),
                ("Runtime", &details.runtime),
            ] {
                if !value.is_empty() {
                    fields = fields.child(field(label, div().child(value.clone()), cx));
                }
            }
        }
        let actions = h_flex().gap_2().child(
            Button::new("show-its-pods")
                .small()
                .outline()
                .icon(Icon::new(IconName::Box))
                .label("Show its pods")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowPods), cx)),
        );
        let Some(details) = details else {
            return v_flex()
                .gap_4()
                .child(fields)
                .child(actions)
                .into_any_element();
        };
        let addresses = details
            .addresses
            .iter()
            .map(|(kind, address)| format!("{kind}  {address}"))
            .collect();
        let resources = details
            .resources
            .iter()
            .map(|(resource, capacity, usable)| format!("{resource}  {capacity}, {usable} usable"))
            .collect();
        let labels = details
            .labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        v_flex()
            .gap_4()
            .child(fields)
            .child(actions)
            .children(mono_section("Addresses", addresses, cx))
            .children(mono_section("Capacity", resources, cx))
            .children(mono_section("Taints", details.taints.clone(), cx))
            .when(!details.conditions.is_empty(), |summary| {
                summary.child(
                    v_flex()
                        .gap_2()
                        .child(section_title("Conditions", cx))
                        .children(
                            details
                                .conditions
                                .iter()
                                .map(|condition| render_condition(condition, cx)),
                        ),
                )
            })
            .children(mono_section("Labels", labels, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{Condition, NodeRow, PodRow, Snapshot};

    use super::{condition_tone, node_tone};
    use crate::table::{PodCounts, ResourceTable, RowMatch, TableRow, Tone};

    fn row(name: &str, status: &str) -> Arc<NodeRow> {
        Arc::new(NodeRow {
            name: name.into(),
            status: status.into(),
            ..NodeRow::default()
        })
    }

    fn pod(name: &str, node: &str) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: "shop".into(),
            name: name.into(),
            node: node.into(),
            ..PodRow::default()
        })
    }

    fn table(rows: Vec<Arc<NodeRow>>) -> ResourceTable<NodeRow> {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn names(table: &ResourceTable<NodeRow>) -> Vec<&str> {
        table.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn each_status_has_a_tone() {
        assert_eq!(node_tone("Ready"), Tone::Healthy);
        assert_eq!(node_tone("Ready, SchedulingDisabled"), Tone::Degraded);
        assert_eq!(node_tone("Ready, DiskPressure"), Tone::Degraded);
        assert_eq!(node_tone("NotReady"), Tone::Failed);
        assert_eq!(node_tone("NotReady, SchedulingDisabled"), Tone::Failed);
        assert_eq!(node_tone("Unknown"), Tone::Failed);
    }

    #[test]
    fn nodes_that_are_not_ready_lead_by_default() {
        let table = table(vec![
            row("a", "Ready"),
            row("b", "Ready, SchedulingDisabled"),
            row("c", "NotReady"),
        ]);
        assert_eq!(names(&table), ["c", "b", "a"]);
    }

    #[test]
    fn a_namespace_does_not_narrow_nodes() {
        let mut table = table(vec![row("a", "Ready")]);
        table.set_namespace(Some("shop".into()));
        assert_eq!(names(&table), ["a"]);
        assert_eq!(table.empty_namespace(), None);
    }

    #[test]
    fn pods_count_and_sort_by_node() {
        let mut table = table(vec![row("a", "Ready"), row("b", "Ready")]);
        let pods = [pod("p1", "b"), pod("p2", "b"), pod("p3", "a")];
        table.set_pod_counts(Arc::new(PodCounts::new(&pods)));

        table.cycle_sort(4);
        assert_eq!(names(&table), ["a", "b"]);
        table.cycle_sort(4);
        assert_eq!(names(&table), ["b", "a"]);
        let counts = PodCounts::new(&pods);
        assert_eq!(table.rows()[0].pod_count(&counts), Some(2));
    }

    #[test]
    fn show_its_pods_narrows_pods_to_the_node_in_every_namespace() {
        let (filter, namespace) = row("node-7", "Ready").pods().unwrap();
        assert_eq!(filter.label, "node/node-7");
        assert_eq!(namespace, None);
        assert!(filter.selector.matches(pod("p1", "node-7").as_ref()));
        assert!(!filter.selector.matches(pod("p2", "node-8").as_ref()));
        assert!(matches!(filter.selector, RowMatch::Field { .. }));
    }

    #[test]
    fn only_ready_is_good_when_true() {
        let condition = |kind: &str, status: &str| Condition {
            kind: kind.into(),
            status: status.into(),
            reason: String::new(),
            message: String::new(),
        };
        assert_eq!(condition_tone(&condition("Ready", "True")), Tone::Healthy);
        assert_eq!(
            condition_tone(&condition("DiskPressure", "True")),
            Tone::Failed
        );
        assert_eq!(
            condition_tone(&condition("DiskPressure", "False")),
            Tone::Healthy
        );
    }
}
