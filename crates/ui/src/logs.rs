use std::ops::Range;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Following, LogLine, LogProgress, LogReader, LogTail, Severity, json_line};

use crate::pods::format_count;

const LINE_HEIGHT: f32 = 20.;
pub const BAR_HEIGHT: f32 = 40.;
const TIME_WIDTH: f32 = 60.;
const SOURCE_WIDTH: f32 = 210.;
const SEVERITY_WIDTH: f32 = 40.;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const DOCK_SHARE: f32 = 0.42;
const MINIMUM_DOCK_HEIGHT: f32 = 160.;

/// How tall the log dock is in a window of this height.
pub fn dock_height(window_height: Pixels) -> Pixels {
    (window_height * DOCK_SHARE).max(px(MINIMUM_DOCK_HEIGHT))
}

/// The newest lines, or a position held still while the user reads.
///
/// Tracks a line number rather than a pixel offset, because `f32` pixels cannot
/// address millions of lines exactly.
pub struct LogPane {
    reader: LogReader,
    progress: LogProgress,
    top: u64,
    following: bool,
    scrolled_lines: f32,
    note: Option<SharedString>,
    _updates: Task<()>,
}

impl LogPane {
    pub fn new(mut tail: LogTail, cx: &mut Context<Self>) -> Self {
        let reader = tail.reader();
        let updates = cx.spawn(async move |this, cx| {
            while let Some(progress) = tail.next().await {
                let updated = this.update(cx, |this, cx| {
                    this.progress = progress;
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
                cx.background_executor().timer(FRAME_INTERVAL).await;
            }
        });
        Self {
            reader,
            progress: LogProgress::default(),
            top: 0,
            following: true,
            scrolled_lines: 0.,
            note: None,
            _updates: updates,
        }
    }

    /// Shows `note` in place of the summary, such as while a workload is found.
    pub fn set_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        self.note = note;
        cx.notify();
    }

    /// The containers this tail has followed, and the lines it holds.
    #[cfg(feature = "bench")]
    pub fn stats(&self) -> (usize, Option<Following>, u64) {
        (
            self.reader.source_count(),
            self.progress.following.clone(),
            self.progress.end,
        )
    }

    fn held(&self) -> Range<u64> {
        self.progress.first..self.progress.end
    }

    pub fn toggle_following(&mut self, cx: &mut Context<Self>) {
        self.set_following(!self.following, cx);
    }

    fn set_following(&mut self, following: bool, cx: &mut Context<Self>) {
        self.following = following;
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollEvent, rows: u64, cx: &mut Context<Self>) {
        let pixels = event.delta.pixel_delta(px(LINE_HEIGHT)).y;
        self.scrolled_lines -= pixels / px(LINE_HEIGHT);
        let lines = self.scrolled_lines.trunc();
        self.scrolled_lines -= lines;
        if lines < 0. {
            self.following = false;
        }
        self.top = scrolled_top(self.top, lines as i64, self.held(), rows);
        cx.notify();
    }

    fn source_label(&self, line: &LogLine) -> String {
        self.reader
            .target(line.source)
            .map(|target| format!("{}/{}", target.pod, target.container))
            .unwrap_or_default()
    }

    fn render_line(&self, line: LogLine, cx: &App) -> impl IntoElement {
        let source = self.source_label(&line);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let time = line
            .timestamp
            .map(|nanos| clock_time(nanos, &Local))
            .unwrap_or_default();
        let severity = line.severity.label();
        let label = [source.as_str(), severity, line.text.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let (text, fields) = match json_line(line.message().as_bytes()) {
            Some(json) => (json.message, json.fields),
            None => (line.message().to_owned(), String::new()),
        };
        let shown = if fields.is_empty() {
            text.clone()
        } else {
            format!("{text} {fields}")
        };
        h_flex()
            .id(("log-line", line.seq))
            .test_support()
            .aria_label(label)
            .h(px(LINE_HEIGHT))
            .gap_3()
            .flex_shrink_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .w(px(TIME_WIDTH))
                    .flex_shrink_0()
                    .text_color(muted)
                    .child(time),
            )
            .child(
                div()
                    .w(px(SOURCE_WIDTH))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .text_color(muted)
                    .child(source),
            )
            .child(
                div()
                    .id(("log-severity", line.seq))
                    .test_support()
                    .aria_label(severity)
                    .w(px(SEVERITY_WIDTH))
                    .flex_shrink_0()
                    .text_color(severity_color(line.severity, theme))
                    .child(severity),
            )
            .child(
                h_flex()
                    .id(("log-text", line.seq))
                    .test_support()
                    .aria_label(shown)
                    .gap_3()
                    .child(text)
                    .when(!fields.is_empty(), |row| {
                        row.child(div().text_color(muted).child(fields))
                    }),
            )
    }
}

/// The wall-clock `HH:MM:SS` of a timestamp in `zone`.
fn clock_time(unix_nanos: i64, zone: &impl TimeZone<Offset: std::fmt::Display>) -> String {
    DateTime::from_timestamp_nanos(unix_nanos)
        .with_timezone(zone)
        .format("%H:%M:%S")
        .to_string()
}

/// Errors, warnings and notices take their status colour. Debug lines and lines with no
/// severity stay muted, and every severity also shows as a word.
fn severity_color(severity: Severity, theme: &Theme) -> Hsla {
    match severity {
        Severity::Error => theme.danger,
        Severity::Warning => theme.warning,
        Severity::Info => theme.info,
        Severity::Debug | Severity::Unknown => theme.muted_foreground,
    }
}

fn line_summary(containers: usize, lines: u64) -> String {
    let containers = match containers {
        1 => "1 container".to_owned(),
        containers => format!("{containers} containers"),
    };
    format!(
        "Tailing {containers}, {} lines kept",
        format_count(lines as usize)
    )
}

/// Names what a follow tails, and says when the pod limit leaves pods out.
fn follow_summary(following: &Following, lines: u64) -> String {
    let pods = match (following.pods, following.matching) {
        (0, _) => "no pods yet".to_owned(),
        (1, 1) => "1 pod".to_owned(),
        (pods, matching) if matching > pods => {
            format!(
                "{pods} of {} pods, the most a tail follows",
                format_count(matching)
            )
        }
        (pods, _) => format!("{pods} pods"),
    };
    format!(
        "Following {} · {pods} · {} lines kept",
        following.label,
        format_count(lines as usize)
    )
}

fn progress_summary(progress: &LogProgress, lines: u64) -> String {
    match &progress.following {
        Some(following) => follow_summary(following, lines),
        None => line_summary(progress.containers, lines),
    }
}

fn last_top(held: &Range<u64>, rows: u64) -> u64 {
    held.end.saturating_sub(rows).max(held.start)
}

fn scrolled_top(top: u64, lines: i64, held: Range<u64>, rows: u64) -> u64 {
    top.saturating_add_signed(lines)
        .clamp(held.start, last_top(&held, rows))
}

impl Render for LogPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines_height = dock_height(window.viewport_size().height) - px(BAR_HEIGHT);
        let rows = (lines_height / px(LINE_HEIGHT)) as u64;
        let held = self.held();
        self.top = if self.following {
            last_top(&held, rows)
        } else {
            scrolled_top(self.top, 0, held.clone(), rows)
        };
        let lines = self.reader.read(self.top..self.top + rows);
        let error = lines
            .as_ref()
            .err()
            .map(|error| format!("Failed to read log lines: {error}"))
            .or_else(|| self.progress.error.clone());
        let summary = self
            .note
            .as_ref()
            .map(ToString::to_string)
            .or(error)
            .unwrap_or_else(|| progress_summary(&self.progress, held.end - held.start));
        let following = self.following;
        v_flex()
            .id("logs")
            .size_full()
            .child(
                h_flex()
                    .h(px(BAR_HEIGHT))
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .child(div().font_medium().child("Logs"))
                    .child(
                        div()
                            .id("log-summary")
                            .test_support()
                            .aria_label(summary.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(summary),
                    )
                    .child(
                        Button::new("log-mode")
                            .small()
                            .ghost()
                            .label(if following { "Pause" } else { "Follow" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_following(!following, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("log-lines")
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .flex()
                    .flex_col()
                    .when(following, |lines| lines.justify_end())
                    .overflow_hidden()
                    .text_size(cx.theme().mono_font_size)
                    .font_family(cx.theme().mono_font_family.clone())
                    .on_scroll(cx.listener(move |this, event, _, cx| {
                        this.scroll(event, rows, cx);
                    }))
                    .children(
                        lines
                            .unwrap_or_default()
                            .into_iter()
                            .map(|line| self.render_line(line, cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use taffrail_core::Following;

    use chrono::{FixedOffset, Utc};

    use super::{clock_time, follow_summary, last_top, line_summary, scrolled_top};

    const NOV_7_2026_10_42_06_NANOS: i64 = 1_794_048_126_118_000_000;

    #[test]
    fn shows_the_time_of_day_in_the_zone_given() {
        assert_eq!(clock_time(NOV_7_2026_10_42_06_NANOS, &Utc), "10:42:06");
        let india = FixedOffset::east_opt(5 * 3_600 + 30 * 60).unwrap();
        assert_eq!(clock_time(NOV_7_2026_10_42_06_NANOS, &india), "16:12:06");
        let new_york = FixedOffset::west_opt(5 * 3_600).unwrap();
        assert_eq!(clock_time(NOV_7_2026_10_42_06_NANOS, &new_york), "05:42:06");
    }

    #[test]
    fn shows_times_before_the_epoch() {
        assert_eq!(clock_time(-1_000_000_000, &Utc), "23:59:59");
    }

    #[test]
    fn summary_counts_containers_and_lines() {
        assert_eq!(line_summary(1, 5), "Tailing 1 container, 5 lines kept");
        assert_eq!(
            line_summary(50, 604_780),
            "Tailing 50 containers, 604,780 lines kept"
        );
    }

    fn following(pods: usize, matching: usize) -> Following {
        Following {
            label: "deployment/checkout-api".into(),
            pods,
            matching,
        }
    }

    #[test]
    fn a_follow_names_its_source_and_counts_pods() {
        assert_eq!(
            follow_summary(&following(6, 6), 1_240),
            "Following deployment/checkout-api · 6 pods · 1,240 lines kept"
        );
        assert_eq!(
            follow_summary(&following(1, 1), 0),
            "Following deployment/checkout-api · 1 pod · 0 lines kept"
        );
    }

    #[test]
    fn a_follow_says_when_it_has_no_pods() {
        assert_eq!(
            follow_summary(&following(0, 0), 12),
            "Following deployment/checkout-api · no pods yet · 12 lines kept"
        );
    }

    #[test]
    fn a_follow_says_when_the_limit_leaves_pods_out() {
        assert_eq!(
            follow_summary(&following(50, 1_200), 5),
            "Following deployment/checkout-api · 50 of 1,200 pods, the most a tail follows · 5 lines kept"
        );
    }

    #[test]
    fn last_top_shows_the_newest_lines() {
        assert_eq!(last_top(&(0..100), 10), 90);
    }

    #[test]
    fn last_top_stays_at_the_oldest_line_when_lines_fit() {
        assert_eq!(last_top(&(50..55), 10), 50);
    }

    #[test]
    fn scrolling_moves_by_whole_lines() {
        assert_eq!(scrolled_top(50, -3, 0..100, 10), 47);
        assert_eq!(scrolled_top(50, 3, 0..100, 10), 53);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        assert_eq!(scrolled_top(2, -5, 0..100, 10), 0);
        assert_eq!(scrolled_top(88, 5, 0..100, 10), 90);
    }

    #[test]
    fn a_held_position_moves_up_when_old_lines_are_dropped() {
        assert_eq!(scrolled_top(10, 0, 500..1_000, 10), 500);
    }

    #[test]
    fn scrolling_stays_exact_past_the_precision_of_f32() {
        assert_eq!(
            scrolled_top(123_456_789, 1, 0..200_000_000, 10),
            123_456_790
        );
    }
}
