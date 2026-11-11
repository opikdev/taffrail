//! The parts of the Summary tab that every workload that runs pods shares.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::Condition;

use crate::details::section_title;
use crate::palette::{ShowPods, TailWorkload};
use crate::table::Tone;

/// **Tail its pods** and **Show its pods**, which need a selector Taffrail can read.
pub fn pod_actions(has_selector: bool) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            Button::new("tail-its-pods")
                .small()
                .outline()
                .icon(Icon::new(IconName::ScrollText))
                .label("Tail its pods")
                .disabled(!has_selector)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(TailWorkload), cx)),
        )
        .child(
            Button::new("show-its-pods")
                .small()
                .outline()
                .icon(Icon::new(IconName::Box))
                .label("Show its pods")
                .disabled(!has_selector)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowPods), cx)),
        )
}

fn render_condition(condition: &Condition, tone: Tone, cx: &App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
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
                        .text_color(tone.color(cx))
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

/// The Conditions section, each coloured by `tone`, or nothing without conditions.
pub fn conditions_section(
    conditions: &[Condition],
    tone: fn(&Condition) -> Tone,
    cx: &App,
) -> Option<AnyElement> {
    (!conditions.is_empty()).then(|| {
        v_flex()
            .gap_2()
            .child(section_title("Conditions", cx))
            .children(
                conditions
                    .iter()
                    .map(|condition| render_condition(condition, tone(condition), cx)),
            )
            .into_any_element()
    })
}

/// A titled list of a name and a muted, monospaced value on each line, such as each
/// container and its image, or nothing when empty.
pub fn pairs_section(
    title: &'static str,
    pairs: impl IntoIterator<Item = (String, String)>,
    cx: &App,
) -> Option<AnyElement> {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let muted = theme.muted_foreground;
    let rows: Vec<_> = pairs
        .into_iter()
        .map(|(name, value)| {
            h_flex()
                .gap_2()
                .text_xs()
                .child(div().flex_shrink_0().child(name))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(mono.clone())
                        .text_color(muted)
                        .child(value),
                )
        })
        .collect();
    (!rows.is_empty()).then(|| {
        v_flex()
            .gap_1()
            .child(section_title(title, cx))
            .children(rows)
            .into_any_element()
    })
}

/// The Labels section, as `key=value` lines, or nothing without labels.
pub fn labels_section(labels: &[(String, String)], cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let muted = theme.muted_foreground;
    (!labels.is_empty()).then(|| {
        v_flex()
            .gap_1()
            .child(section_title("Labels", cx))
            .children(labels.iter().map(|(key, value)| {
                div()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .child(format!("{key}={value}"))
            }))
            .into_any_element()
    })
}

/// A selector or other machine text, in the monospaced font.
pub fn mono_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs()
        .child(text.into())
}

/// Whether a condition reads as healthy, for a kind whose conditions are good when true.
pub fn condition_tone(condition: &Condition) -> Tone {
    match condition.status.as_str() {
        "True" => Tone::Healthy,
        "False" => Tone::Failed,
        _ => Tone::Neutral,
    }
}
