use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Condition, DeploymentDetails, DeploymentRow, Follow};

use crate::details::{Inspect, field, section_title};
use crate::palette::{ShowPods, TailWorkload};
use crate::table::{
    CellStyle, ColumnDef, TableRow, Tone, age_text, by_age, format_age, unix_now, urgency,
};

pub fn deployment_tone(status: &str) -> Tone {
    match status {
        "Available" => Tone::Healthy,
        "Progressing" => Tone::Degraded,
        "Paused" | "ScaledToZero" => Tone::Neutral,
        _ => Tone::Failed,
    }
}

/// A count out of the replicas the Deployment wants, such as `2/3`.
fn of_desired(count: i32, row: &DeploymentRow) -> SharedString {
    format!("{count}/{}", row.desired).into()
}

impl TableRow for DeploymentRow {
    const SINGULAR: &'static str = "deployment";
    const PLURAL: &'static str = "deployments";
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
            name: "Available",
            width: 84.,
            style: CellStyle::Number,
            text: |row, _| row.available.to_string().into(),
            compare: |a, b| a.available.cmp(&b.available),
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
        deployment_tone(&self.status)
    }

    fn follow(&self) -> Option<Follow> {
        DeploymentRow::follow(self)
    }
}

/// Whether a condition reads as healthy. `ReplicaFailure` is the one that is bad when true.
fn condition_tone(condition: &Condition) -> Tone {
    let bad_when_true = condition.kind == "ReplicaFailure";
    match condition.status.as_str() {
        "True" if bad_when_true => Tone::Failed,
        "True" => Tone::Healthy,
        "False" if bad_when_true => Tone::Healthy,
        "False" => Tone::Failed,
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
        .when(!condition.reason.is_empty(), |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(condition.reason.clone()),
            )
        })
        .when(!condition.message.is_empty(), |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(condition.message.clone()),
            )
        })
}

impl Inspect for DeploymentRow {
    type Details = DeploymentDetails;

    fn summary(&self, details: Option<&DeploymentDetails>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let muted = theme.muted_foreground;
        let mut fields = v_flex().gap_1p5();
        fields = fields.child(field(
            "Status",
            div()
                .text_color(self.tone().color(cx))
                .child(self.status.clone()),
            cx,
        ));
        fields = fields.child(field(
            "Ready",
            div().child(of_desired(self.ready, self)),
            cx,
        ));
        fields = fields.child(field(
            "Up-to-date",
            div().child(self.updated.to_string()),
            cx,
        ));
        fields = fields.child(field(
            "Available",
            div().child(self.available.to_string()),
            cx,
        ));
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
                    div()
                        .font_family(mono.clone())
                        .text_xs()
                        .child(details.selector.clone()),
                    cx,
                ));
        }
        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("tail-its-pods")
                    .small()
                    .outline()
                    .icon(Icon::new(IconName::ScrollText))
                    .label("Tail its pods")
                    .disabled(self.selector.is_none())
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(TailWorkload), cx)),
            )
            .child(
                Button::new("show-its-pods")
                    .small()
                    .outline()
                    .icon(Icon::new(IconName::Box))
                    .label("Show its pods")
                    .disabled(self.selector.is_none())
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowPods), cx)),
            );
        let conditions = details.map_or(&[][..], |details| &details.conditions);
        let images = details.map_or(&[][..], |details| &details.images);
        let labels = details.map_or(&[][..], |details| &details.labels);
        v_flex()
            .gap_4()
            .child(fields)
            .child(actions)
            .when(!conditions.is_empty(), |summary| {
                summary.child(
                    v_flex()
                        .gap_2()
                        .child(section_title("Conditions", cx))
                        .children(
                            conditions
                                .iter()
                                .map(|condition| render_condition(condition, cx)),
                        ),
                )
            })
            .when(!images.is_empty(), |summary| {
                summary.child(
                    v_flex()
                        .gap_1()
                        .child(section_title("Images", cx))
                        .children(images.iter().map(|(container, image)| {
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(div().flex_shrink_0().child(container.clone()))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(mono.clone())
                                        .text_color(muted)
                                        .child(image.clone()),
                                )
                        })),
                )
            })
            .when(!labels.is_empty(), |summary| {
                summary.child(
                    v_flex()
                        .gap_1()
                        .child(section_title("Labels", cx))
                        .children(labels.iter().map(|(key, value)| {
                            div()
                                .text_xs()
                                .font_family(mono.clone())
                                .text_color(muted)
                                .child(format!("{key}={value}"))
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{Condition, DeploymentRow, Snapshot};

    use super::{condition_tone, deployment_tone};
    use crate::table::{ResourceTable, Tone};

    fn row(name: &str, status: &str, ready: i32) -> Arc<DeploymentRow> {
        Arc::new(DeploymentRow {
            namespace: "shop".into(),
            name: name.into(),
            status: status.into(),
            desired: 3,
            ready,
            ..DeploymentRow::default()
        })
    }

    fn table(rows: Vec<Arc<DeploymentRow>>) -> ResourceTable<DeploymentRow> {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn names(table: &ResourceTable<DeploymentRow>) -> Vec<&str> {
        table.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn stalled_and_unavailable_rollouts_lead_by_default() {
        let table = table(vec![
            row("a", "Available", 3),
            row("b", "Progressing", 2),
            row("c", "Stalled", 1),
            row("d", "ScaledToZero", 0),
            row("e", "Unavailable", 0),
        ]);
        assert_eq!(names(&table), ["c", "e", "b", "a", "d"]);
    }

    #[test]
    fn ready_sorts_by_ready_replicas() {
        let mut table = table(vec![row("a", "Available", 3), row("b", "Available", 1)]);
        table.cycle_sort(3);
        assert_eq!(names(&table), ["b", "a"]);
    }

    #[test]
    fn each_status_has_a_tone() {
        assert_eq!(deployment_tone("Available"), Tone::Healthy);
        assert_eq!(deployment_tone("Progressing"), Tone::Degraded);
        assert_eq!(deployment_tone("Paused"), Tone::Neutral);
        assert_eq!(deployment_tone("ReplicaFailure"), Tone::Failed);
    }

    #[test]
    fn a_replica_failure_is_bad_when_true() {
        let condition = |kind: &str, status: &str| Condition {
            kind: kind.into(),
            status: status.into(),
            reason: String::new(),
            message: String::new(),
        };
        assert_eq!(
            condition_tone(&condition("Available", "True")),
            Tone::Healthy
        );
        assert_eq!(
            condition_tone(&condition("Available", "False")),
            Tone::Failed
        );
        assert_eq!(
            condition_tone(&condition("ReplicaFailure", "True")),
            Tone::Failed
        );
    }
}
