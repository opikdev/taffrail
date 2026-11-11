use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{CoreEventDetails, CoreEventRow, EventDetails, EventRow, ObjectLink};

use crate::details::{Inspect, field, object_link, section_title};
use crate::table::{
    CellStyle, ColumnDef, Jump, TableRow, Tone, age_text, by_age, format_age, unix_now,
};
use crate::views::View;

/// Warnings are degraded, so they lead. Normal events are only a record.
pub fn event_tone(row: &EventRow) -> Tone {
    if row.warning {
        Tone::Degraded
    } else {
        Tone::Neutral
    }
}

/// The object an event is about, as a link. A kind from a dotted API group, such as a
/// custom resource, could share a built-in kind's name, so it gets none.
pub fn event_link(row: &EventRow) -> Option<ObjectLink> {
    let custom = row.object_group.contains('.');
    (!custom && !row.object_kind.is_empty()).then(|| {
        ObjectLink::new(
            row.object_kind.clone(),
            row.object_namespace.clone(),
            row.object_name.clone(),
        )
    })
}

/// Where `Enter` on an event goes: the object it is about, if that kind has a view.
pub fn event_jump(row: &EventRow) -> Option<Jump> {
    let link = event_link(row)?;
    View::for_kind(&link.kind)?;
    Some(Jump::Link(link))
}

/// The Summary tab of an event, read from either API.
fn event_summary(row: &EventRow, details: Option<&EventDetails>, cx: &App) -> AnyElement {
    let ago = |seen: i64| format!("{} ago", format_age(unix_now() - seen));
    let mut fields = v_flex().gap_1p5();
    fields = fields
        .child(field(
            "Type",
            div()
                .text_color(event_tone(row).color(cx))
                .child(row.kind()),
            cx,
        ))
        .child(field("Reason", div().child(row.reason.clone()), cx))
        .child(field(
            "Object",
            match event_link(row) {
                Some(link) => v_flex()
                    .items_start()
                    .child(object_link(&link, row.object())),
                None => div().child(row.object()),
            },
            cx,
        ));
    if !row.object_namespace.is_empty() {
        fields = fields.child(field(
            "Namespace",
            div().child(row.object_namespace.clone()),
            cx,
        ));
    }
    fields = fields.child(field("Count", div().child(row.count.to_string()), cx));
    if let Some(seen) = row.last_seen_unix {
        fields = fields.child(field("Last seen", div().child(ago(seen)), cx));
    }
    if let Some(details) = details {
        if let Some(seen) = details.first_seen_unix {
            fields = fields.child(field("First seen", div().child(ago(seen)), cx));
        }
        for (label, value) in [
            ("Source", Some(&details.source)),
            ("Action", Some(&details.action)),
            ("Related", details.related.as_ref()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                fields = fields.child(field(label, div().child(value.clone()), cx));
            }
        }
        fields = fields.child(field("API", div().child(details.api), cx));
    }
    let message = details.map_or(&row.message, |details| &details.message);
    v_flex()
        .gap_4()
        .child(fields)
        .child(
            v_flex().gap_1().child(section_title("Message", cx)).child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(message.clone()),
            ),
        )
        .into_any_element()
}

/// Both event rows show the same table and Summary, whichever API they came from.
macro_rules! event_view {
    ($row:ty, $details:ty) => {
        impl TableRow for $row {
            const SINGULAR: &'static str = "event";
            const PLURAL: &'static str = "events";
            const COLUMNS: &'static [ColumnDef<Self>] = &[
                ColumnDef {
                    name: "Last seen",
                    width: 84.,
                    style: CellStyle::Muted,
                    text: |row, now| age_text(row.last_seen_unix, now),
                    compare: |a, b| by_age(a.last_seen_unix, b.last_seen_unix),
                },
                ColumnDef {
                    name: "Type",
                    width: 100.,
                    style: CellStyle::Status,
                    text: |row, _| row.kind().into(),
                    compare: |a, b| b.warning.cmp(&a.warning),
                },
                ColumnDef {
                    name: "Reason",
                    width: 170.,
                    style: CellStyle::Text,
                    text: |row, _| row.reason.clone().into(),
                    compare: |a, b| a.reason.cmp(&b.reason),
                },
                ColumnDef {
                    name: "Object",
                    width: 260.,
                    style: CellStyle::Text,
                    text: |row, _| row.object().into(),
                    compare: |a, b| {
                        (&a.object_kind, &a.object_name).cmp(&(&b.object_kind, &b.object_name))
                    },
                },
                ColumnDef {
                    name: "Message",
                    width: 520.,
                    style: CellStyle::Muted,
                    text: |row, _| row.message.clone().into(),
                    compare: |a, b| a.message.cmp(&b.message),
                },
                ColumnDef {
                    name: "Count",
                    width: 64.,
                    style: CellStyle::Number,
                    text: |row, _| row.count.to_string().into(),
                    compare: |a, b| a.count.cmp(&b.count),
                },
                ColumnDef {
                    name: "Namespace",
                    width: 120.,
                    style: CellStyle::Muted,
                    text: |row, _| row.namespace.clone().into(),
                    compare: |a, b| a.namespace.cmp(&b.namespace),
                },
            ];

            fn tone(&self) -> Tone {
                event_tone(self)
            }

            fn jump(&self) -> Option<Jump> {
                event_jump(self)
            }

            fn default_rank(&self) -> i64 {
                -self.last_seen_unix.unwrap_or(0)
            }
        }

        impl Inspect for $row {
            type Details = $details;

            fn summary(&self, details: Option<&$details>, cx: &App) -> AnyElement {
                event_summary(self, details.map(AsRef::as_ref), cx)
            }
        }
    };
}

event_view!(EventRow, EventDetails);
event_view!(CoreEventRow, CoreEventDetails);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::{EventRow, ObjectLink, Snapshot};

    use super::event_jump;
    use crate::table::{Jump, ResourceTable};

    fn row(name: &str, warning: bool, seen: i64) -> Arc<EventRow> {
        Arc::new(EventRow {
            namespace: "shop".into(),
            name: name.into(),
            warning,
            reason: "BackOff".into(),
            object_kind: "Pod".into(),
            object_name: "api-1".into(),
            object_namespace: "shop".into(),
            last_seen_unix: Some(seen),
            ..EventRow::default()
        })
    }

    #[test]
    fn warnings_lead_and_newer_events_come_first_within_a_type() {
        let mut table = ResourceTable::default();
        table.show(Arc::new(Snapshot {
            rows: vec![
                row("a", false, 300),
                row("b", true, 100),
                row("c", false, 400),
                row("d", true, 200),
            ],
            synced: true,
            error: None,
        }));
        let names: Vec<_> = table.rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["d", "b", "c", "a"]);
    }

    #[test]
    fn an_event_leads_to_its_object_only_if_that_kind_has_a_view() {
        let pod = row("a", true, 1);
        assert_eq!(
            event_jump(&pod),
            Some(Jump::Link(ObjectLink::new("Pod", "shop", "api-1")))
        );
        assert_eq!(event_jump(&pod).unwrap().label(), "Go to pod/api-1");

        let mut service = (*pod).clone();
        service.object_kind = "Service".into();
        assert_eq!(event_jump(&service), None);

        let mut custom = (*pod).clone();
        custom.object_kind = "Deployment".into();
        custom.object_group = "example.com".into();
        assert_eq!(event_jump(&custom), None);
    }
}
