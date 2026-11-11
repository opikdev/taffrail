use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Icon, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{
    Backend, ClusterError, Connection, ContainerDetails, ObjectDetails, ObjectEvent, ObjectLink,
    PodDetails, PodRow,
};

use crate::palette::OpenLink;
use crate::pods::status_tone;
use crate::table::{TableRow, Tone, format_age, unix_now};
use crate::views::View;

pub const DETAILS_WIDTH: f32 = 380.;

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const LABEL_WIDTH: f32 = 84.;
const EVENT_DOT: f32 = 6.;
const YAML_LINE_HEIGHT: f32 = 20.;
const TABS: [PaneTab; 3] = [PaneTab::Summary, PaneTab::Events, PaneTab::Yaml];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaneTab {
    Summary,
    Events,
    Yaml,
}

pub enum DetailsEvent {
    Close,
}

/// A kind of object the details panel can show.
pub trait Inspect: TableRow {
    type Details: ObjectDetails;

    /// The Summary tab, from the row and, once read, the details.
    fn summary(&self, details: Option<&Self::Details>, cx: &App) -> AnyElement;
}

/// `pod` as `Pod`, for titles.
pub fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// One object's summary, events and YAML, kept current while the panel is open.
pub struct DetailsPane<R: Inspect> {
    backend: Arc<Backend>,
    connection: Connection,
    row: Arc<R>,
    gone: bool,
    tab: PaneTab,
    details: Option<Result<R::Details, SharedString>>,
    yaml_lines: Rc<Vec<SharedString>>,
    yaml_scroll: UniformListScrollHandle,
    _refresh: Option<Task<()>>,
}

impl<R: Inspect> EventEmitter<DetailsEvent> for DetailsPane<R> {}

impl<R: Inspect> DetailsPane<R> {
    pub fn new(
        backend: Arc<Backend>,
        connection: Connection,
        row: Arc<R>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            backend,
            connection,
            row,
            gone: false,
            tab: PaneTab::Summary,
            details: None,
            yaml_lines: Rc::default(),
            yaml_scroll: UniformListScrollHandle::new(),
            _refresh: None,
        };
        pane.refresh(cx);
        pane
    }

    pub fn show_tab(&mut self, tab: PaneTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Takes the latest row for this object, and reads it again if it changed.
    pub fn update_row(&mut self, row: Arc<R>, cx: &mut Context<Self>) {
        if self.row != row {
            self.row = row;
            self.refresh(cx);
            cx.notify();
        }
    }

    /// The object left the cluster. What was read stays visible, marked as old.
    pub fn mark_gone(&mut self, cx: &mut Context<Self>) {
        if !self.gone {
            self.gone = true;
            self._refresh = None;
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let connection = self.connection.clone();
        let (namespace, name) = self.row.key();
        // Replacing the task drops a read in progress, so an older answer cannot win.
        self._refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                let details = backend
                    .details::<R::Details>(&connection, namespace.clone(), name.clone())
                    .await;
                if this.update(cx, |this, cx| this.show(details, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        }));
    }

    fn show(&mut self, details: Result<R::Details, ClusterError>, cx: &mut Context<Self>) {
        let details = details.map_err(|error| match error {
            ClusterError::ReadObject { error, .. } => SharedString::from(error),
            error => SharedString::from(error.to_string()),
        });
        if self.details.as_ref() == Some(&details) {
            return;
        }
        if let Ok(details) = &details {
            self.yaml_lines = Rc::new(
                details
                    .yaml()
                    .lines()
                    .map(|line| line.to_owned().into())
                    .collect(),
            );
        }
        self.details = Some(details);
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<R> {
        let muted = cx.theme().muted_foreground;
        let events = self
            .details
            .as_ref()
            .and_then(|details| details.as_ref().ok()?.events().as_ref().ok())
            .map_or(0, Vec::len);
        let event_label: SharedString = match events {
            0 => "Events".into(),
            events => format!("Events {events}").into(),
        };
        let selected = TABS.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        v_flex()
            .flex_shrink_0()
            .px_3()
            .pt_2()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("details-title")
                            .test_support()
                            .role(Role::Heading)
                            .aria_label(self.row.name().to_owned())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(self.row.name().to_owned()),
                    )
                    .child(
                        Button::new("close-details")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::X))
                            .accessibility_label("Close details")
                            .tooltip("Close details")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DetailsEvent::Close))),
                    ),
            )
            .when(self.gone, |header| {
                header.child(div().text_xs().text_color(muted).child(format!(
                    "This {} no longer exists. Showing what was last read.",
                    R::SINGULAR
                )))
            })
            .child(
                TabBar::new("details-tabs")
                    .underline()
                    .small()
                    .selected_index(selected)
                    .child(Tab::new().label("Summary"))
                    .child(Tab::new().label(event_label))
                    .child(Tab::new().label("YAML"))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.tab = TABS[*ix];
                        cx.notify();
                    })),
            )
    }

    fn render_events(&self, events: &Result<Vec<ObjectEvent>, String>, cx: &App) -> AnyElement {
        match events {
            Ok(events) if events.is_empty() => message(
                "No recent events",
                Some("Kubernetes usually keeps events for about an hour."),
                cx,
            ),
            Ok(events) => v_flex()
                .gap_3()
                .children(events.iter().map(|event| render_event(event, cx)))
                .into_any_element(),
            Err(error) => message(
                &format!("Can't list events for this {}", R::SINGULAR),
                Some(error),
                cx,
            ),
        }
    }

    fn render_yaml(&self, cx: &mut Context<Self>) -> impl IntoElement + use<R> {
        let lines = self.yaml_lines.clone();
        let count = lines.len();
        let yaml = lines
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join("\n");
        let font = cx.theme().mono_font_family.clone();
        let size = cx.theme().mono_font_size;
        v_flex()
            .size_full()
            .child(
                h_flex().px_3().py_1().justify_end().child(
                    Button::new("copy-yaml")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Copy))
                        .label("Copy YAML")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(yaml.clone()));
                        })),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .font_family(font)
                    .text_size(size)
                    .child(
                        uniform_list(
                            "yaml-lines",
                            count,
                            cx.processor(move |_, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|ix| lines.get(ix).cloned())
                                    .map(|line| render_yaml_line(line, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.yaml_scroll)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .size_full(),
                    ),
            )
    }
}

impl Inspect for PodRow {
    type Details = PodDetails;

    fn summary(&self, details: Option<&PodDetails>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let status_color = status_tone(&self.status).color(cx);
        let age = self
            .created_unix
            .map(|created| format_age(unix_now() - created));
        let restarts = self.restarts.to_string();
        let mut fields = v_flex().gap_1p5();
        fields = fields.child(field(
            "Status",
            div().text_color(status_color).child(self.status.clone()),
            cx,
        ));
        fields = fields.child(field("Ready", div().child(self.ready.clone()), cx));
        fields = fields.child(field("Restarts", div().child(restarts), cx));
        fields = fields.child(field("Namespace", div().child(self.namespace.clone()), cx));
        if !self.node.is_empty() {
            let node = ObjectLink::new("Node", "", self.node.clone());
            let text = self.node.clone();
            fields = fields.child(field("Node", div().child(object_link(&node, text)), cx));
        }
        if let Some(age) = age {
            fields = fields.child(field("Age", div().child(age), cx));
        }
        if let Some(owner) = owner_field(self, details, cx) {
            fields = fields.child(field("Owner", owner, cx));
        }
        if let Some(ip) = details.and_then(|details| details.pod_ip.clone()) {
            fields = fields.child(field("Pod IP", div().child(ip), cx));
        }
        let containers = details.map_or(&[][..], |details| &details.containers);
        let labels = details.map_or(&[][..], |details| &details.labels);
        v_flex()
            .gap_4()
            .child(fields)
            .when(!containers.is_empty(), |summary| {
                summary.child(
                    v_flex()
                        .gap_2()
                        .child(section_title("Containers", cx))
                        .children(
                            containers
                                .iter()
                                .map(|container| render_container(container, cx)),
                        ),
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
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(format!("{key}={value}"))
                        })),
                )
            })
            .into_any_element()
    }
}

/// The pod's controller, led by the Deployment behind it once that is read, such as
/// **deployment/api** via **replicaset/api-7f9c**.
fn owner_field(pod: &PodRow, details: Option<&PodDetails>, cx: &App) -> Option<Div> {
    let muted = cx.theme().muted_foreground;
    let controller = match details {
        Some(details) => details.controller.clone(),
        None => pod
            .owner
            .as_ref()
            .map(|owner| ObjectLink::new(&owner.kind, &pod.namespace, &owner.name)),
    }?;
    let workload = details.and_then(|details| details.workload.clone());
    let owner = match workload {
        Some(Ok(workload)) => v_flex()
            .items_start()
            .child(object_link(&workload, workload.label()))
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child("via")
                    .child(object_link(&controller, controller.label())),
            ),
        Some(Err(reason)) => v_flex()
            .items_start()
            .child(object_link(&controller, controller.label()))
            .child(div().text_xs().text_color(muted).child(format!(
                "Can't read the {} to find its Deployment. {reason}",
                controller.kind
            ))),
        None => v_flex()
            .items_start()
            .child(object_link(&controller, controller.label())),
    };
    Some(owner)
}

/// An object another names, as a link to it in the view for its kind, or as plain text
/// while its kind has no view. Following it works from the keyboard too, with `Tab` and
/// `Enter`.
pub fn object_link(link: &ObjectLink, text: impl Into<SharedString>) -> AnyElement {
    let text = text.into();
    if View::for_kind(&link.kind).is_none() {
        return div()
            .id(SharedString::from(format!("text {}", link.label())))
            .test_support()
            .aria_label(text.clone())
            .min_w_0()
            .truncate()
            .child(text)
            .into_any_element();
    }
    let target = link.clone();
    Button::new(SharedString::from(format!("link {}", link.label())))
        .link()
        .small()
        .compact()
        .label(text)
        .accessibility_label(format!("Go to {}", link.label()))
        .on_click(move |_, window, cx| {
            window.dispatch_action(
                Box::new(OpenLink {
                    link: target.clone(),
                }),
                cx,
            );
        })
        .into_any_element()
}

pub fn field(label: &'static str, value: Div, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .items_start()
        .text_sm()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(value.flex_1().min_w_0())
}

pub fn section_title(title: &'static str, cx: &App) -> impl IntoElement + use<> {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

pub fn message(title: &str, detail: Option<&str>, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .text_sm()
        .child(title.to_owned())
        .when_some(detail, |content, detail| {
            content.child(
                div()
                    .id("details-message")
                    .test_support()
                    .aria_label(detail.to_owned())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail.to_owned()),
            )
        })
        .into_any_element()
}

fn render_container(container: &ContainerDetails, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let color = status_tone(&container.state).color(cx);
    let ready = if container.ready {
        "Ready"
    } else {
        "Not ready"
    };
    v_flex()
        .gap_0p5()
        .p_2()
        .rounded(theme.radius)
        .bg(theme.popover)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(container.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .child(container.state.clone()),
                ),
        )
        .child(
            div()
                .text_xs()
                .truncate()
                .text_color(muted)
                .font_family(theme.mono_font_family.clone())
                .child(container.image.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{ready}, {} restarts", container.restarts)),
        )
        .when_some(container.last_exit.clone(), |card, exit| {
            card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Last exit: {exit}")),
            )
        })
}

fn render_event(event: &ObjectEvent, cx: &App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    let tone = if event.warning {
        Tone::Degraded
    } else {
        Tone::Neutral
    };
    let age = event
        .last_seen_unix
        .map(|seen| format_age(unix_now() - seen))
        .unwrap_or_default();
    let meta = match event.count {
        count if count > 1 => format!("{age}, {count} times"),
        _ => age,
    };
    v_flex()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().size(px(EVENT_DOT)).rounded_full().bg(tone.color(cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(event.reason.clone()),
                )
                .child(div().text_xs().text_color(muted).child(meta)),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(event.message.clone()),
        )
}

/// Splits a YAML line into its indent and list dashes, its key, and its value.
fn split_yaml_line(line: &str) -> (&str, Option<&str>, &str) {
    let body = line.trim_start_matches([' ', '-']);
    let lead = &line[..line.len() - body.len()];
    if body.starts_with(['"', '\'']) {
        return (lead, None, body);
    }
    match body.split_once(": ") {
        Some((key, _)) => (lead, Some(key), &body[key.len()..]),
        None => match body.strip_suffix(':') {
            Some(key) => (lead, Some(key), ":"),
            None => (lead, None, body),
        },
    }
}

fn render_yaml_line(line: SharedString, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (lead, key, value) = split_yaml_line(&line);
    h_flex()
        .h(px(YAML_LINE_HEIGHT))
        .whitespace_nowrap()
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(lead.to_owned()),
        )
        .when_some(key, |row, key| {
            row.child(div().text_color(theme.info).child(key.to_owned()))
        })
        .child(div().child(value.to_owned()))
}

impl<R: Inspect> Render for DetailsPane<R> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let details = self.details.clone();
        let body = match (&details, self.tab) {
            (Some(Err(error)), _) if !self.gone => scrollable(
                "details-error",
                message(&format!("Can't read this {}", R::SINGULAR), Some(error), cx),
            ),
            (Some(Ok(_)), PaneTab::Yaml) => self.render_yaml(cx).into_any_element(),
            (Some(Ok(details)), PaneTab::Events) => {
                scrollable("details-events", self.render_events(details.events(), cx))
            }
            (details, PaneTab::Summary) => {
                let details = details.as_ref().and_then(|details| details.as_ref().ok());
                scrollable("details-summary", self.row.summary(details, cx))
            }
            _ => scrollable("details-loading", message("Loading", None, cx)),
        };
        v_flex()
            .id("details")
            .test_support()
            .role(Role::Complementary)
            .aria_label(format!("{} details", capitalized(R::SINGULAR)))
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn scrollable(id: &'static str, content: AnyElement) -> AnyElement {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .p_3()
        .child(content)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::split_yaml_line;

    #[test]
    fn splits_a_key_from_its_value() {
        assert_eq!(
            split_yaml_line("  name: api-1"),
            ("  ", Some("name"), ": api-1")
        );
    }

    #[test]
    fn keeps_a_key_with_nothing_after_it() {
        assert_eq!(split_yaml_line("spec:"), ("", Some("spec"), ":"));
    }

    #[test]
    fn treats_list_dashes_as_indent() {
        assert_eq!(
            split_yaml_line("  - name: app"),
            ("  - ", Some("name"), ": app")
        );
    }

    #[test]
    fn leaves_plain_values_without_a_key() {
        assert_eq!(split_yaml_line("  - tcp"), ("  - ", None, "tcp"));
    }

    #[test]
    fn does_not_read_a_quoted_value_as_a_key() {
        assert_eq!(split_yaml_line("  - \"a: b\""), ("  - ", None, "\"a: b\""));
    }
}
