use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{NamespaceDetails, NamespaceRow};

use crate::details::{Inspect, field, section_title};
use crate::palette::FollowSelected;
use crate::table::{
    CellStyle, ColumnDef, Jump, PodCounts, TableRow, Tone, age_text, by_age, format_age, unix_now,
    urgency,
};

pub fn namespace_tone(status: &str) -> Tone {
    match status {
        "Active" => Tone::Healthy,
        "Terminating" => Tone::Degraded,
        _ => Tone::Neutral,
    }
}

impl TableRow for NamespaceRow {
    const SINGULAR: &'static str = "namespace";
    const PLURAL: &'static str = "namespaces";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 320.,
            style: CellStyle::Text,
            text: |row, _| row.name.clone().into(),
            compare: |a, b| a.name.cmp(&b.name),
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
        namespace_tone(&self.status)
    }

    fn pod_count(&self, counts: &PodCounts) -> Option<usize> {
        Some(counts.by_namespace.get(&self.name).copied().unwrap_or(0))
    }

    fn jump(&self) -> Option<Jump> {
        Some(Jump::Namespace(self.name.clone()))
    }
}

impl Inspect for NamespaceRow {
    type Details = NamespaceDetails;

    fn summary(&self, details: Option<&NamespaceDetails>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (mono, muted) = (theme.mono_font_family.clone(), theme.muted_foreground);
        let mut fields = v_flex().gap_1p5();
        fields = fields.child(field(
            "Status",
            div()
                .text_color(self.tone().color(cx))
                .child(self.status.clone()),
            cx,
        ));
        if let Some(created) = self.created_unix {
            fields = fields.child(field(
                "Age",
                div().child(format_age(unix_now() - created)),
                cx,
            ));
        }
        if let Some(details) = details {
            fields = fields.child(field(
                "Annotations",
                div().child(details.annotations.to_string()),
                cx,
            ));
            if !details.finalizers.is_empty() {
                fields = fields.child(field(
                    "Finalizers",
                    div().child(details.finalizers.join(", ")),
                    cx,
                ));
            }
        }
        let label = Jump::Namespace(self.name.clone()).label();
        let actions = h_flex().gap_2().child(
            Button::new("use-namespace")
                .small()
                .outline()
                .icon(Icon::new(IconName::Folder))
                .label(label)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(FollowSelected), cx)),
        );
        let conditions = details.map_or(&[][..], |details| &details.conditions);
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
                        .children(conditions.iter().map(|condition| {
                            v_flex()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_sm()
                                        .child(
                                            div().flex_1().min_w_0().child(condition.kind.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(condition.status.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(condition.message.clone()),
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

    use taffrail_core::{NamespaceRow, PodRow, Snapshot};

    use super::namespace_tone;
    use crate::table::{Jump, PodCounts, ResourceTable, TableRow, Tone};

    fn row(name: &str, status: &str) -> Arc<NamespaceRow> {
        Arc::new(NamespaceRow {
            name: name.into(),
            status: status.into(),
            created_unix: None,
        })
    }

    #[test]
    fn terminating_namespaces_lead_by_default() {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows: vec![row("a", "Active"), row("b", "Terminating")],
            synced: true,
            error: None,
        }));
        let names: Vec<_> = table.rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
        assert_eq!(namespace_tone("Active"), Tone::Healthy);
        assert_eq!(namespace_tone("Terminating"), Tone::Degraded);
    }

    #[test]
    fn pods_count_by_namespace_and_enter_chooses_it() {
        let pod = Arc::new(PodRow {
            namespace: "shop".into(),
            name: "api-1".into(),
            ..PodRow::default()
        });
        let counts = PodCounts::new(&[pod]);
        assert_eq!(row("shop", "Active").pod_count(&counts), Some(1));
        assert_eq!(row("web", "Active").pod_count(&counts), Some(0));
        let jump = row("shop", "Active").jump().unwrap();
        assert_eq!(jump, Jump::Namespace("shop".into()));
        assert_eq!(jump.label(), "Use namespace shop");
    }
}
