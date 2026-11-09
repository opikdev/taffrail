use std::ops::Range;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{ActiveTheme, Icon, Sizable, StyledExt, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{
    FilterError, FilterProgress, Following, Gap, LogFilter, LogFilterScan, LogLine, LogProgress,
    LogReader, LogTail, Severity, json_line,
};

use crate::pods::format_count;

const LINE_HEIGHT: f32 = 20.;
pub const BAR_HEIGHT: f32 = 40.;
const TIME_WIDTH: f32 = 60.;
const SOURCE_WIDTH: f32 = 210.;
const SEVERITY_WIDTH: f32 = 40.;
const GAP_INSET: f32 = 8.;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const DOCK_SHARE: f32 = 0.42;
const MINIMUM_DOCK_HEIGHT: f32 = 160.;
const FILTER_WIDTH: f32 = 220.;
const FILTER_MIN_WIDTH: f32 = 96.;
const SEVERITY_PICKER_WIDTH: f32 = 136.;
const NOTICE_WIDTH: f32 = 420.;
/// The key context around the log filter field, where `Esc` clears it.
pub const LOG_FILTER_CONTEXT: &str = "LogFilter";

/// The choices of the severity picker, lowest first.
pub const SEVERITY_CHOICES: [(&str, Severity); 4] = [
    ("All levels", Severity::Unknown),
    ("INFO and above", Severity::Info),
    ("WARN and above", Severity::Warning),
    ("ERROR only", Severity::Error),
];

pub type SeverityPicker = SelectState<Vec<&'static str>>;

/// The severity a picker choice stands for.
pub fn chosen_severity(choice: Option<&&'static str>) -> Severity {
    SEVERITY_CHOICES
        .iter()
        .find(|(label, _)| Some(label) == choice)
        .map_or(Severity::Unknown, |(_, severity)| *severity)
}

/// The filter field and severity picker, which outlive any one tail.
#[derive(Clone)]
pub struct LogControls {
    pub query: Entity<InputState>,
    pub severity: Entity<SeverityPicker>,
}

actions!(taffrail, [ResetLogFilter]);

/// A filter and the scan that finds its lines.
struct ActiveFilter {
    filter: LogFilter,
    scan: Result<LogFilterScan, FilterError>,
    progress: FilterProgress,
    _updates: Option<Task<()>>,
}

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
    /// The first line in view. With a filter, the first matching line in view.
    top: u64,
    following: bool,
    scrolled_lines: f32,
    note: Option<SharedString>,
    controls: LogControls,
    filter: Option<ActiveFilter>,
    _updates: Task<()>,
}

impl LogPane {
    pub fn new(
        mut tail: LogTail,
        controls: LogControls,
        filter: LogFilter,
        cx: &mut Context<Self>,
    ) -> Self {
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
        let mut pane = Self {
            reader,
            progress: LogProgress::default(),
            top: 0,
            following: true,
            scrolled_lines: 0.,
            note: None,
            controls,
            filter: None,
            _updates: updates,
        };
        pane.set_filter(filter, cx);
        pane
    }

    /// Narrows the lines to those `filter` matches, across every stored line. A held
    /// position stays on the same line.
    pub fn set_filter(&mut self, filter: LogFilter, cx: &mut Context<Self>) {
        let current = self.filter.as_ref().map(|active| &active.filter);
        if current == Some(&filter) || (current.is_none() && filter.is_empty()) {
            return;
        }
        if filter.is_empty() {
            self.filter = None;
            cx.notify();
            return;
        }
        let scan = self.reader.filter(&filter);
        let progress = scan
            .as_ref()
            .map(LogFilterScan::progress)
            .unwrap_or_default();
        let updates = scan.as_ref().ok().map(|scan| {
            let mut updates = scan.updates();
            cx.spawn(async move |this, cx| {
                while let Some(progress) = updates.next().await {
                    let updated = this.update(cx, |this, cx| {
                        if let Some(active) = &mut this.filter {
                            active.progress = progress;
                        }
                        cx.notify();
                    });
                    if updated.is_err() {
                        return;
                    }
                    cx.background_executor().timer(FRAME_INTERVAL).await;
                }
            })
        });
        self.filter = Some(ActiveFilter {
            filter,
            scan,
            progress,
            _updates: updates,
        });
        cx.notify();
    }

    /// Shows `note` in place of the summary, such as while a workload is found.
    pub fn set_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        self.note = note;
        cx.notify();
    }

    /// The containers this tail has followed, the lines it holds, and a reader for them.
    #[cfg(feature = "bench")]
    pub fn stats(&self) -> (usize, Option<Following>, u64, LogReader) {
        (
            self.reader.source_count(),
            self.progress.following.clone(),
            self.progress.end,
            self.reader.clone(),
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
        match self.filter.as_ref().map(|active| &active.scan) {
            Some(Ok(scan)) => {
                let total = scan.len() as u64;
                let position = scan.position_of(self.top) as u64;
                let position = scrolled_top(position, lines as i64, 0..total, rows) as usize;
                if let Some(top) = scan.seqs(position..position + 1).first() {
                    self.top = *top;
                }
            }
            Some(Err(_)) => {}
            None => self.top = scrolled_top(self.top, lines as i64, self.held(), rows),
        }
        cx.notify();
    }

    /// The lines in view, and where they start among the matches when a filter is on.
    fn visible_lines(&mut self, rows: u64) -> std::io::Result<Vec<LogLine>> {
        let held = self.held();
        let Some(active) = &self.filter else {
            self.top = if self.following {
                last_top(&held, rows)
            } else {
                scrolled_top(self.top, 0, held.clone(), rows)
            };
            return self.reader.read(self.top..self.top + rows);
        };
        let Ok(scan) = &active.scan else {
            return Ok(Vec::new());
        };
        let total = scan.len() as u64;
        let position = if self.following {
            last_top(&(0..total), rows)
        } else {
            scrolled_top(scan.position_of(self.top) as u64, 0, 0..total, rows)
        } as usize;
        let lines = scan.lines(position..position + rows as usize)?;
        if let Some(first) = lines.first() {
            self.top = first.seq;
        }
        Ok(lines)
    }

    /// Says how many lines match, or why the filter can't run.
    fn render_count(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let active = self.filter.as_ref()?;
        let theme = cx.theme();
        let (label, color) = match &active.scan {
            Ok(scan) => (
                match_count(scan.len(), self.held(), active.progress.complete),
                theme.muted_foreground,
            ),
            Err(error) => (error.to_string(), theme.danger),
        };
        Some(
            div()
                .id("log-filter-count")
                .test_support()
                .aria_label(label.clone())
                .flex_shrink_0()
                .text_color(color)
                .child(label),
        )
    }

    /// The **No matches** state, or why the filter can't run.
    fn render_filter_notice(&self) -> Option<impl IntoElement + use<>> {
        let active = self.filter.as_ref()?;
        let title = match &active.scan {
            Ok(scan) if scan.is_empty() && active.progress.complete => {
                no_matches_title(&active.filter)
            }
            Ok(_) => return None,
            Err(error) => error.to_string(),
        };
        Some(
            v_flex()
                .id("log-notice")
                .test_support()
                .aria_label(title.clone())
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .text_sm()
                .child(
                    div()
                        .max_w(px(NOTICE_WIDTH))
                        .text_center()
                        .font_medium()
                        .child(title),
                )
                .child(
                    Button::new("log-clear-filter")
                        .small()
                        .outline()
                        .label("Clear filter")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ResetLogFilter), cx);
                        }),
                ),
        )
    }

    fn source_label(&self, line: &LogLine) -> String {
        self.reader
            .target(line.source)
            .map(|target| format!("{}/{}", target.pod, target.container))
            .unwrap_or_default()
    }

    /// A full-width row in place of the columns, so a gap reads apart from lines.
    fn render_gap(&self, line: &LogLine, gap: Gap, cx: &App) -> AnyElement {
        let source = self.source_label(line);
        let text = gap_text(gap, &Local);
        let theme = cx.theme();
        h_flex()
            .id(("log-line", line.seq))
            .test_support()
            .aria_label(format!("{source} {text}"))
            .h(px(LINE_HEIGHT))
            .flex_shrink_0()
            .gap_3()
            // The band reaches into the padding, so its text lines up with the times.
            .mx(px(-GAP_INSET))
            .px(px(GAP_INSET))
            .rounded_sm()
            .whitespace_nowrap()
            .overflow_hidden()
            .bg(theme.muted)
            .text_color(theme.muted_foreground)
            .child(text)
            .child(source)
            .into_any_element()
    }

    fn render_line(&self, line: LogLine, cx: &App) -> AnyElement {
        if let Some(gap) = line.gap {
            return self.render_gap(&line, gap, cx);
        }
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
            .into_any_element()
    }
}

/// What a gap row says, with times of day in `zone`.
pub(crate) fn gap_text(gap: Gap, zone: &impl TimeZone<Offset: std::fmt::Display>) -> String {
    format!(
        "Lines may be missing from {} to {}",
        clock_time(gap.from, zone),
        clock_time(gap.to, zone)
    )
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

/// How many stored lines a filter has found, such as `312 of 48,210 lines`.
fn match_count(matches: usize, held: Range<u64>, complete: bool) -> String {
    let held = format_count((held.end - held.start) as usize);
    let matches = format_count(matches);
    if complete {
        format!("{matches} of {held} lines")
    } else {
        format!("{matches} of {held} lines so far")
    }
}

/// The **No matches** title for a filter that found nothing.
fn no_matches_title(filter: &LogFilter) -> String {
    let severity = match filter.severity {
        Severity::Unknown => "",
        Severity::Debug => " at DEBUG and above",
        Severity::Info => " at INFO and above",
        Severity::Warning => " at WARN and above",
        Severity::Error => " at ERROR",
    };
    let query = filter.query.trim();
    if query.is_empty() {
        format!("No lines{severity}")
    } else {
        format!("No lines match \"{query}\"{severity}")
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
        let lines = self.visible_lines(rows);
        let error = lines
            .as_ref()
            .err()
            .map(|error| format!("Failed to read log lines: {error}"))
            .or_else(|| self.progress.error.clone())
            .or_else(|| {
                let active = self.filter.as_ref()?;
                active.progress.error.clone()
            });
        let summary = self
            .note
            .as_ref()
            .map(ToString::to_string)
            .or(error)
            .unwrap_or_else(|| progress_summary(&self.progress, held.end - held.start));
        let following = self.following;
        let muted = cx.theme().muted_foreground;
        let count = self.render_count(cx);
        let notice = self.render_filter_notice();
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
                    .child(div().flex_shrink_0().font_medium().child("Logs"))
                    .child(
                        div()
                            .id("log-summary")
                            .test_support()
                            .aria_label(summary.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(summary),
                    )
                    .children(count)
                    .child(
                        div().w(px(SEVERITY_PICKER_WIDTH)).flex_shrink_0().child(
                            Select::new(&self.controls.severity)
                                .id("log-severity-picker")
                                .small()
                                .accessibility_label("Severity"),
                        ),
                    )
                    .child(
                        div()
                            .key_context(LOG_FILTER_CONTEXT)
                            .w(px(FILTER_WIDTH))
                            .min_w(px(FILTER_MIN_WIDTH))
                            .child(
                                Input::new(&self.controls.query)
                                    .id("log-filter")
                                    .small()
                                    .prefix(Icon::new(IconName::Search).small().text_color(muted))
                                    .aria_label("Filter log lines"),
                            ),
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
            .map(|dock| match notice {
                Some(notice) => dock.child(notice).into_any_element(),
                None => dock
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
                    .into_any_element(),
            })
    }
}

#[cfg(test)]
mod tests {
    use taffrail_core::{Following, Gap};

    use chrono::{FixedOffset, Utc};

    use taffrail_core::{LogFilter, Severity};

    use super::{
        chosen_severity, clock_time, follow_summary, gap_text, last_top, line_summary, match_count,
        no_matches_title, scrolled_top,
    };

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
    fn a_gap_names_the_time_range_that_may_be_missing() {
        let gap = Gap {
            from: NOV_7_2026_10_42_06_NANOS,
            to: NOV_7_2026_10_42_06_NANOS + 25_000_000_000,
        };

        assert_eq!(
            gap_text(gap, &Utc),
            "Lines may be missing from 10:42:06 to 10:42:31"
        );
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
    fn the_count_says_how_many_stored_lines_match() {
        assert_eq!(match_count(312, 0..48_210, true), "312 of 48,210 lines");
        assert_eq!(
            match_count(12, 100..1_100, false),
            "12 of 1,000 lines so far"
        );
    }

    #[test]
    fn no_matches_names_the_filter() {
        let filter = |query: &str, severity| LogFilter {
            query: query.into(),
            severity,
        };
        assert_eq!(
            no_matches_title(&filter(" timeout ", Severity::Unknown)),
            "No lines match \"timeout\""
        );
        assert_eq!(
            no_matches_title(&filter("timeout", Severity::Warning)),
            "No lines match \"timeout\" at WARN and above"
        );
        assert_eq!(
            no_matches_title(&filter("", Severity::Error)),
            "No lines at ERROR"
        );
    }

    #[test]
    fn each_severity_choice_names_its_lowest_severity() {
        assert_eq!(chosen_severity(Some(&"WARN and above")), Severity::Warning);
        assert_eq!(chosen_severity(Some(&"All levels")), Severity::Unknown);
        assert_eq!(chosen_severity(None), Severity::Unknown);
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
