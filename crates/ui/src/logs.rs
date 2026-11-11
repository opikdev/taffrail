use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, StyledExt, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{
    CONTEXT_SEPARATOR, ExportProgress, FilterError, FilterProgress, Following, Gap, Level,
    LogExport, LogFilter, LogFilterScan, LogLine, LogProgress, LogReader, LogTail, Severity,
    TimelineView, export_directory, json_line,
};

use crate::palette::ExportLines;
use crate::rows::{BlockCache, Row, RowId, RowKind, Rows};
use crate::table::format_count;
use crate::timeline::{
    BARS_HEIGHT, LOG_TIMELINE_CONTEXT, TIMELINE_HEIGHT, TimelineNext, TimelinePrevious, axis_time,
    bucket_label, duration_label, level_color, max_buckets, next_bucket, segment_heights,
};

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
const COMPACT_FILTER_WIDTH: f32 = 120.;
const SEVERITY_PICKER_WIDTH: f32 = 136.;
const NOTICE_WIDTH: f32 = 420.;
/// Room left of the log dock for the sidebar and the panel's inset, used to size the
/// timeline before its first paint.
const DOCK_INSET: f32 = 240.;
/// Below this window width the dock header drops its title and narrows its controls,
/// and the timeline's legend leaves out the bar length, so both fit the 760 px minimum.
const COMPACT_DOCK_BELOW: f32 = 920.;
const SWATCH_SIZE: f32 = 8.;
/// The accent edge of the line that `n` or `Shift+N` found.
const MARK_WIDTH: f32 = 2.;
/// The column that marks matched lines while context shows.
const MATCH_MARK_WIDTH: f32 = 8.;
const CONTEXT_PICKER_WIDTH: f32 = 148.;
const COMPACT_CONTEXT_PICKER_WIDTH: f32 = 92.;
const COMPACT_SEVERITY_PICKER_WIDTH: f32 = 116.;
/// The most lines one copy puts on the clipboard. An export has no limit.
const MAX_COPY_LINES: usize = 100_000;
/// The file name the save dialog suggests for an export.
const EXPORT_NAME: &str = "taffrail-logs.log";
/// The key context around the log filter field, where `Esc` clears it.
pub const LOG_FILTER_CONTEXT: &str = "LogFilter";
/// The key context of the focused log lines, where arrows move the selection.
pub const LOG_LINES_CONTEXT: &str = "LogLines";

/// The choices of the severity picker, lowest first.
pub const SEVERITY_CHOICES: [(&str, Severity); 4] = [
    ("All levels", Severity::Unknown),
    ("INFO and above", Severity::Info),
    ("WARN and above", Severity::Warning),
    ("ERROR only", Severity::Error),
];

pub type SeverityPicker = SelectState<Vec<&'static str>>;

/// The choices of the context picker: how many lines of a match's source show around it.
pub const CONTEXT_CHOICES: [(&str, usize); 3] = [("None", 0), ("3 lines", 3), ("10 lines", 10)];

pub type LineContextPicker = SelectState<Vec<&'static str>>;

/// The number of lines a context picker choice stands for.
pub fn chosen_context(choice: Option<&&'static str>) -> usize {
    CONTEXT_CHOICES
        .iter()
        .find(|(label, _)| Some(label) == choice)
        .map_or(0, |(_, lines)| *lines)
}

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
    pub context: Entity<LineContextPicker>,
}

actions!(
    taffrail,
    [
        ResetLogFilter,
        SelectNextLine,
        SelectPreviousLine,
        ExtendNextLine,
        ExtendPreviousLine,
        ClearLineSelection
    ]
);

/// Selected rows, from `anchor` to `head`. The head moves with the arrow keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: RowId,
    head: RowId,
    /// The line at the head, which `n` and `Shift+N` look from.
    head_seq: u64,
}

impl Selection {
    fn single(id: RowId, seq: u64) -> Self {
        Self {
            anchor: id,
            head: id,
            head_seq: seq,
        }
    }

    fn range(&self) -> (RowId, RowId) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    fn contains(&self, id: RowId) -> bool {
        let (first, last) = self.range();
        (first..=last).contains(&id)
    }
}

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
    /// The first row in view.
    top: RowId,
    following: bool,
    scrolled_lines: f32,
    note: Option<SharedString>,
    controls: LogControls,
    filter: Option<ActiveFilter>,
    /// The first and last lines in view at the last render, and how many rows fit.
    visible: Option<(u64, u64)>,
    rows: u64,
    /// The time of the line the view is at, for the timeline to mark.
    view_time: Option<i64>,
    selection: Option<Selection>,
    /// Lines of a match's source shown around it, while a filter is on.
    context: usize,
    blocks: BlockCache,
    lines_focus: FocusHandle,
    /// Says what `n`, a copy or an export did, until the view moves.
    hint: Option<SharedString>,
    /// The save dialog, then the export it starts. Dropping it stops the export.
    export: Option<Task<()>>,
    timeline: TimelineView,
    /// Set when lines or matches change, so the bars are counted again.
    timeline_stale: bool,
    timeline_buckets: usize,
    /// The strip's width at its last paint.
    timeline_width: Rc<Cell<Pixels>>,
    timeline_focus: FocusHandle,
    /// The bar that the timeline's arrow keys move from, by the time it starts.
    cursor: Option<i64>,
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
                    this.timeline_stale = true;
                    this.blocks.clear();
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
            top: RowId::default(),
            following: true,
            scrolled_lines: 0.,
            note: None,
            controls,
            filter: None,
            visible: None,
            rows: 0,
            view_time: None,
            selection: None,
            context: 0,
            blocks: BlockCache::default(),
            lines_focus: cx.focus_handle().tab_stop(true),
            hint: None,
            export: None,
            timeline: TimelineView::default(),
            timeline_stale: true,
            timeline_buckets: 0,
            timeline_width: Rc::new(Cell::new(px(0.))),
            timeline_focus: cx.focus_handle().tab_stop(true),
            cursor: None,
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
        self.timeline_stale = true;
        self.hint = None;
        self.selection = None;
        self.blocks.clear();
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
                        this.timeline_stale = true;
                        this.blocks.clear();
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

    /// Shows `lines` lines of each match's source around it, while a filter is on.
    pub fn set_context(&mut self, lines: usize, cx: &mut Context<Self>) {
        if self.context == lines {
            return;
        }
        self.context = lines;
        self.selection = None;
        self.blocks.clear();
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
        self.hint = None;
        if following {
            self.selection = None;
            self.cursor = None;
        }
        cx.notify();
    }

    /// The matching scan, or `None` without a filter. A filter that can't run has no lines.
    fn scan(&self) -> Option<Result<&LogFilterScan, &FilterError>> {
        self.filter.as_ref().map(|active| active.scan.as_ref())
    }

    /// What the dock shows now.
    fn rows(&mut self) -> Rows<'_> {
        let held = self.progress.first..self.progress.end;
        match self.filter.as_ref().map(|active| &active.scan) {
            None => Rows::All {
                reader: &self.reader,
                held,
            },
            Some(Err(_)) => Rows::Nothing,
            Some(Ok(scan)) if self.context == 0 => Rows::Matches(scan),
            Some(Ok(scan)) => Rows::Context {
                scan,
                lines: self.context,
                cache: &mut self.blocks,
            },
        }
    }

    /// Holds the view still with the row `id`, `above` rows below the top.
    fn show_row(&mut self, id: RowId, above: u64) {
        self.following = false;
        self.top = self.rows().step(id, -(above as i64)).unwrap_or(id);
    }

    /// Moves to the next error, or the previous one, that the filter shows, and selects
    /// it. With nothing selected, it looks from the lines in view, including them.
    pub fn jump_to_error(&mut self, forward: bool, cx: &mut Context<Self>) {
        let from = match (self.selection, self.visible, forward) {
            (Some(selection), _, true) => selection.head_seq + 1,
            (Some(selection), _, false) => selection.head_seq,
            (None, Some((first, _)), true) => first,
            (None, Some((_, last)), false) => last + 1,
            (None, None, _) => self.progress.end,
        };
        let found = match self.scan() {
            Some(Ok(scan)) if forward => scan.next_error(from),
            Some(Ok(scan)) => scan.previous_error(from),
            Some(Err(_)) => None,
            None if forward => self.reader.next_error(from),
            None => self.reader.previous_error(from),
        };
        match found {
            Some(seq) => {
                let id = self.rows().row_of(seq).unwrap_or(RowId::line(seq));
                self.selection = Some(Selection::single(id, seq));
                self.cursor = None;
                self.hint = None;
                self.show_row(id, self.rows / 3);
            }
            None => {
                let hint = match (forward, self.filter.is_some()) {
                    (true, false) => "No later errors",
                    (false, false) => "No earlier errors",
                    (true, true) => "No later errors match the filter",
                    (false, true) => "No earlier errors match the filter",
                };
                self.hint = Some(hint.into());
            }
        }
        cx.notify();
    }

    /// Selects the row `id`, or extends the selection to it, and pauses following.
    fn select_row(&mut self, id: RowId, seq: u64, extend: bool, cx: &mut Context<Self>) {
        self.selection = Some(match self.selection {
            Some(selection) if extend => Selection {
                head: id,
                head_seq: seq,
                ..selection
            },
            _ => Selection::single(id, seq),
        });
        self.following = false;
        self.hint = None;
        cx.notify();
    }

    /// Moves the selection's head by `delta` rows, past separators, and keeps it in view.
    fn move_selection(&mut self, delta: i64, extend: bool, cx: &mut Context<Self>) {
        let rows = self.rows;
        let (start, start_seq) = match self.selection {
            Some(selection) => (selection.head, selection.head_seq),
            None => {
                // The first arrow selects a line at the edge of the view it points into.
                let Ok(view) = self.visible_rows(rows) else {
                    return;
                };
                let edge = if delta > 0 {
                    view.iter().find_map(|row| Some((row.id, row.line()?.seq)))
                } else {
                    view.iter()
                        .rev()
                        .find_map(|row| Some((row.id, row.line()?.seq)))
                };
                if let Some((id, seq)) = edge {
                    self.select_row(id, seq, false, cx);
                }
                return;
            }
        };
        let mut id = start;
        let mut seq = start_seq;
        let mut target = self.rows().step(start, delta).unwrap_or(start);
        // A separator is not a line, so the head steps over it.
        for _ in 0..2 {
            let row = self
                .rows()
                .from(target, 1)
                .ok()
                .and_then(|rows| rows.into_iter().next());
            match row {
                Some(Row {
                    id: found,
                    kind: RowKind::Line { line, .. },
                }) => {
                    id = found;
                    seq = line.seq;
                    break;
                }
                Some(_) => {
                    let next = self.rows().step(target, delta.signum()).unwrap_or(target);
                    if next == target {
                        break;
                    }
                    target = next;
                }
                None => break,
            }
        }
        self.select_row(id, seq, extend, cx);
        self.keep_in_view(id);
    }

    /// Scrolls just enough to show the row `id`.
    fn keep_in_view(&mut self, id: RowId) {
        let rows = self.rows.max(1);
        if id < self.top {
            self.top = id;
            return;
        }
        let top = self.top;
        let bottom = self.rows().step(top, rows as i64 - 1).unwrap_or(top);
        if id > bottom {
            self.top = self.rows().step(id, -(rows as i64 - 1)).unwrap_or(id);
        }
    }

    fn clear_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.take().is_none() {
            window.blur(cx);
        }
        cx.notify();
    }

    /// Copies the selected lines as they are stored, each after its source, as
    /// `kubectl logs --prefix --timestamps` prints them.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            self.hint = Some("Select lines to copy them".into());
            cx.notify();
            return;
        };
        let (first, last) = selection.range();
        let rows = self.rows().between(first, last, MAX_COPY_LINES + 1);
        let mut rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                self.hint = Some(format!("Failed to read log lines: {error}").into());
                cx.notify();
                return;
            }
        };
        let cut = rows.len() > MAX_COPY_LINES;
        rows.truncate(MAX_COPY_LINES);
        let mut lines = 0;
        let text: Vec<String> = rows
            .iter()
            .map(|row| match &row.kind {
                RowKind::Line { line, .. } => {
                    lines += 1;
                    self.reader.raw_line(line)
                }
                RowKind::Separator => CONTEXT_SEPARATOR.to_owned(),
            })
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n")));
        self.hint = Some(
            if cut {
                format!(
                    "Copied the first {} lines. Export to save them all",
                    format_count(lines)
                )
            } else if lines == 1 {
                "Copied 1 line".to_owned()
            } else {
                format!("Copied {} lines", format_count(lines))
            }
            .into(),
        );
        cx.notify();
    }

    /// Asks where to save the lines the dock shows, then writes them there: every stored
    /// line, or the filter's matches with any context.
    pub fn export_lines(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_new_path(&export_directory(), Some(EXPORT_NAME));
        self.export = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Ok(Some(mut export)) = this.update(cx, |this, _| this.start_export(path)) else {
                return;
            };
            loop {
                let progress = export.progress();
                let hint = export_hint(&progress, &name);
                if this
                    .update(cx, |this, cx| {
                        this.hint = Some(hint.into());
                        cx.notify();
                    })
                    .is_err()
                    || progress.finished
                {
                    return;
                }
                if export.next().await.is_none() {
                    return;
                }
                cx.background_executor().timer(FRAME_INTERVAL).await;
            }
        }));
    }

    /// Starts writing what the dock shows to `path`.
    fn start_export(&mut self, path: std::path::PathBuf) -> Option<LogExport> {
        match self.scan() {
            None => Some(self.reader.export(path)),
            Some(Ok(scan)) => Some(scan.export(self.context, path)),
            Some(Err(_)) => None,
        }
    }

    /// Shows the first line of the bar at `index`, and pauses following.
    fn jump_to_bucket(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(bucket) = self.timeline.buckets.get(index) else {
            return;
        };
        let Some(first) = bucket.first else {
            return;
        };
        self.cursor = Some(self.timeline.bucket_start(index));
        self.hint = None;
        let id = self.rows().row_of(first).unwrap_or(RowId::line(first));
        self.show_row(id, 0);
        cx.notify();
    }

    /// The bar the view is at: the one the arrow keys moved to, or the one that holds the
    /// line in view. Following has none.
    fn current_bucket(&self) -> Option<usize> {
        if self.following {
            return None;
        }
        self.timeline.bucket_at(self.cursor.or(self.view_time)?)
    }

    fn move_on_timeline(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(index) = next_bucket(&self.timeline, self.current_bucket(), forward) {
            self.jump_to_bucket(index, cx);
        }
    }

    /// Counts the bars again when lines, matches or the strip's width have changed.
    fn update_timeline(&mut self, window: &Window) {
        let measured = self.timeline_width.get();
        let width = if measured > px(0.) {
            measured
        } else {
            window.viewport_size().width - px(DOCK_INSET)
        };
        let buckets = max_buckets(width);
        if !self.timeline_stale && buckets == self.timeline_buckets {
            return;
        }
        self.timeline = match self.scan() {
            Some(Ok(scan)) => scan.timeline(buckets),
            Some(Err(_)) => TimelineView::default(),
            None => self.reader.timeline(buckets),
        };
        self.timeline_stale = false;
        self.timeline_buckets = buckets;
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let pixels = event.delta.pixel_delta(px(LINE_HEIGHT)).y;
        self.scrolled_lines -= pixels / px(LINE_HEIGHT);
        let lines = self.scrolled_lines.trunc();
        self.scrolled_lines -= lines;
        if lines < 0. {
            self.following = false;
        }
        if lines != 0. {
            self.cursor = None;
            self.hint = None;
        }
        if lines != 0. {
            let top = self.top;
            self.top = self.rows().step(top, lines as i64).unwrap_or(top);
        }
        cx.notify();
    }

    /// The rows in view: the newest while following, or those from the held position.
    fn visible_rows(&mut self, rows: u64) -> std::io::Result<Vec<Row>> {
        let (following, top) = (self.following, self.top);
        let rows = rows as usize;
        let view = if following {
            self.rows().last(rows)?
        } else {
            self.rows().view(top, rows)?
        };
        if let Some(first) = view.first() {
            self.top = first.id;
        }
        Ok(view)
    }

    /// Says how many lines match, or why the filter can't run.
    fn render_count(&self, compact: bool, cx: &App) -> Option<impl IntoElement + use<>> {
        let active = self.filter.as_ref()?;
        let theme = cx.theme();
        let (label, color) = match &active.scan {
            Ok(scan) => (
                match_count(scan.len(), self.held(), active.progress.complete),
                theme.muted_foreground,
            ),
            Err(error) => (error.to_string(), theme.danger),
        };
        // A narrow header shows `312 of 48,210`, and the label still reads in full.
        let shown = if compact {
            label.replace(" lines", "")
        } else {
            label.clone()
        };
        Some(
            div()
                .id("log-filter-count")
                .test_support()
                .aria_label(label)
                .min_w_0()
                .truncate()
                .text_color(color)
                .child(shown),
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

    /// A row of the dock: a line, a gap, or a separator between blocks of context.
    fn render_row(&self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id;
        let RowKind::Line { line, matched } = row.kind else {
            return self.render_separator(id, cx);
        };
        let selected = self
            .selection
            .is_some_and(|selection| selection.contains(id));
        let head = self.selection.is_some_and(|selection| selection.head == id);
        let seq = line.seq;
        let element = match line.gap {
            Some(gap) => self.render_gap(&line, gap, cx),
            None => self.render_line(line, matched, cx),
        };
        let theme = cx.theme();
        element
            .aria_selected(selected)
            // The band reaches into the padding, so its text lines up with the times.
            .mx(px(-GAP_INSET))
            .pr(px(GAP_INSET))
            .pl(px(if head {
                GAP_INSET - MARK_WIDTH
            } else {
                GAP_INSET
            }))
            .when(selected, |row| row.bg(theme.table_active))
            .when(head, |row| {
                row.border_l(px(MARK_WIDTH)).border_color(theme.ring)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.lines_focus.focus(window, cx);
                    this.select_row(id, seq, event.modifiers.shift, cx);
                }),
            )
            .test_support()
            .into_any_element()
    }

    /// Marks lines of a source skipped between two blocks of context.
    fn render_separator(&self, id: RowId, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .id(("log-separator", id.key))
            .test_support()
            .aria_label("Lines skipped")
            .h(px(LINE_HEIGHT))
            .flex_shrink_0()
            .gap_3()
            .items_center()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .w(px(TIME_WIDTH))
                    .flex_shrink_0()
                    .child(CONTEXT_SEPARATOR),
            )
            .child(div().flex_1().h(px(1.)).bg(theme.border))
            .into_any_element()
    }

    /// A full-width row in place of the columns, so a gap reads apart from lines.
    fn render_gap(&self, line: &LogLine, gap: Gap, cx: &App) -> Stateful<Div> {
        let source = self.source_label(line);
        let text = gap_text(gap, &Local);
        let theme = cx.theme();
        h_flex()
            .id(("log-line", line.seq))
            .aria_label(format!("{source} {text}"))
            .h(px(LINE_HEIGHT))
            .flex_shrink_0()
            .gap_3()
            .rounded_sm()
            .whitespace_nowrap()
            .overflow_hidden()
            .bg(theme.muted)
            .text_color(theme.muted_foreground)
            .child(text)
            .child(source)
    }

    /// A line's columns. With context, `matched` marks the line the filter matched, and
    /// the lines around it are muted.
    fn render_line(&self, line: LogLine, matched: Option<bool>, cx: &App) -> Stateful<Div> {
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
        let around = matched == Some(false);
        h_flex()
            .id(("log-line", line.seq))
            .aria_label(label)
            .when_some(matched, |row, matched| {
                row.aria_description(if matched { "Match" } else { "Around a match" })
            })
            .h(px(LINE_HEIGHT))
            .gap_3()
            .flex_shrink_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .when_some(matched, |row, matched| {
                row.child(
                    div()
                        .w(px(MATCH_MARK_WIDTH))
                        .flex_shrink_0()
                        .when(matched, |mark| mark.child("›")),
                )
            })
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
                    .child(
                        div()
                            .when(around, |text| text.text_color(muted))
                            .child(text),
                    )
                    .when(!fields.is_empty(), |row| {
                        row.child(div().text_color(muted).child(fields))
                    }),
            )
    }
}

impl LogPane {
    /// The strip of bars above the lines, with its times and legend.
    fn render_timeline(
        &self,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let view = &self.timeline;
        let tallest = view
            .buckets
            .iter()
            .map(|bucket| bucket.counts.total())
            .max()
            .unwrap_or(0);
        let focused = self.timeline_focus.is_focused(window);
        let current = self.current_bucket();
        let area = BARS_HEIGHT - 4.;
        let gap_pattern = pattern_slash(theme.muted_foreground.opacity(0.4), 1., 3.);
        let bars: Vec<AnyElement> = view
            .buckets
            .iter()
            .enumerate()
            .map(|(index, bucket)| {
                let label: SharedString = bucket_label(view, index, &Local).into();
                let heights = segment_heights(&bucket.counts, tallest, area);
                let is_current = current == Some(index);
                div()
                    .id(("log-timeline-bar", index))
                    .test_support()
                    .aria_label(label.clone())
                    .aria_selected(is_current)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .border_t_2()
                    .border_color(if is_current && focused {
                        theme.ring
                    } else {
                        transparent_black()
                    })
                    .when(bucket.gap, |bar| bar.bg(gap_pattern))
                    .when(is_current && !bucket.gap, |bar| bar.bg(theme.table_active))
                    .children(Level::ALL.iter().rev().filter_map(|level| {
                        let height = heights[*level as usize];
                        (height > 0.).then(|| {
                            div()
                                .h(px(height))
                                .flex_shrink_0()
                                .bg(level_color(*level, theme))
                        })
                    }))
                    .when(bucket.first.is_some(), |bar| {
                        bar.cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.timeline_focus.focus(window, cx);
                                this.jump_to_bucket(index, cx);
                            }))
                    })
                    .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                    .into_any_element()
            })
            .collect();
        let width = self.timeline_width.clone();
        let measure = canvas(
            move |bounds, window, cx| {
                if (bounds.size.width - width.get()).abs() >= px(1.) {
                    width.set(bounds.size.width);
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        handle.update(cx, |_, window, _| window.refresh()).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let muted = theme.muted_foreground;
        let totals = view.totals();
        let legend = Level::ALL
            .iter()
            .map(|level| {
                let color = level_color(*level, theme);
                let word = if *level == Level::Other { muted } else { color };
                legend_item(
                    div().size(px(SWATCH_SIZE)).rounded_sm().bg(color),
                    level.label(),
                    word,
                    Some(format_count(totals.get(*level) as usize)),
                )
            })
            .chain(view.has_gaps().then(|| {
                legend_item(
                    div()
                        .size(px(SWATCH_SIZE))
                        .rounded_sm()
                        .border_1()
                        .border_color(muted)
                        .bg(gap_pattern),
                    "MISSING",
                    muted,
                    None,
                )
            }))
            .collect::<Vec<_>>();
        let (start, end, summary) = match view.buckets.len() {
            0 => (
                String::new(),
                String::new(),
                "Timeline, no lines".to_owned(),
            ),
            count => {
                let start = axis_time(view.start, view, &Local);
                let end = axis_time(view.bucket_start(count), view, &Local);
                let per_bar = duration_label(view.width);
                let summary = format!("Timeline from {start} to {end}, {per_bar} per bar");
                (start, end, summary)
            }
        };
        let per_bar =
            (!compact && view.width > 0).then(|| format!("{} per bar", duration_label(view.width)));
        v_flex()
            .id("log-timeline")
            .test_support()
            .track_focus(&self.timeline_focus)
            .key_context(LOG_TIMELINE_CONTEXT)
            .aria_label(summary)
            .on_action(cx.listener(|this, _: &TimelinePrevious, _, cx| {
                this.move_on_timeline(false, cx);
            }))
            .on_action(cx.listener(|this, _: &TimelineNext, _, cx| {
                this.move_on_timeline(true, cx);
            }))
            .h(px(TIMELINE_HEIGHT))
            .flex_shrink_0()
            .mx_3()
            .pt_1()
            .gap_1()
            .child(
                h_flex()
                    .id("log-timeline-bars")
                    .relative()
                    .h(px(BARS_HEIGHT))
                    .flex_shrink_0()
                    .items_end()
                    .gap(px(1.))
                    .border_b_1()
                    .border_color(if focused && view.buckets.is_empty() {
                        theme.ring
                    } else {
                        theme.border
                    })
                    .child(measure)
                    .children(bars),
            )
            .child(
                h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .whitespace_nowrap()
                    .child(div().flex_shrink_0().child(start))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .gap_3()
                            .overflow_hidden()
                            .children(legend)
                            .children(per_bar),
                    )
                    .child(div().flex_shrink_0().child(end)),
            )
    }
}

/// A legend entry: a swatch, the word, and how many lines it counts.
fn legend_item(swatch: Div, word: &'static str, color: Hsla, count: Option<String>) -> AnyElement {
    h_flex()
        .flex_shrink_0()
        .gap_1()
        .items_center()
        .child(swatch)
        .child(div().text_color(color).child(word))
        .children(count)
        .into_any_element()
}

/// What the dock header says while an export runs, and after.
fn export_hint(progress: &ExportProgress, name: &str) -> String {
    if let Some(error) = &progress.error {
        return error.clone();
    }
    if progress.finished {
        return match progress.lines {
            1 => format!("Exported 1 line to {name}"),
            lines => format!("Exported {} lines to {name}", format_count(lines as usize)),
        };
    }
    let percent = (progress.done * 100)
        .checked_div(progress.total)
        .unwrap_or_default();
    format!("Exporting to {name}: {percent}%")
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

impl Render for LogPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines_height =
            dock_height(window.viewport_size().height) - px(BAR_HEIGHT + TIMELINE_HEIGHT);
        let rows = (lines_height / px(LINE_HEIGHT)) as u64;
        self.rows = rows;
        let held = self.held();
        let lines = self.visible_rows(rows);
        if let Ok(lines) = &lines {
            let seq = |row: &Row| Some(row.line()?.seq);
            let first = lines.iter().find_map(seq);
            let last = lines.iter().rev().find_map(seq);
            self.visible = first.zip(last);
            let head = self.selection.map(|selection| selection.head);
            let selected = lines
                .iter()
                .find(|row| Some(row.id) == head)
                .and_then(|row| row.line()?.timestamp);
            self.view_time =
                selected.or_else(|| lines.iter().find_map(|row| row.line()?.timestamp));
        }
        self.update_timeline(window);
        let compact = window.viewport_size().width < px(COMPACT_DOCK_BELOW);
        let timeline = self.render_timeline(compact, window, cx);
        let error = lines
            .as_ref()
            .err()
            .map(|error| format!("Failed to read log lines: {error}"))
            .or_else(|| self.progress.error.clone())
            .or_else(|| {
                let active = self.filter.as_ref()?;
                active.progress.error.clone()
            })
            .or_else(|| self.hint.as_ref().map(ToString::to_string));
        let summary = self
            .note
            .as_ref()
            .map(ToString::to_string)
            .or(error)
            .unwrap_or_else(|| progress_summary(&self.progress, held.end - held.start));
        let following = self.following;
        let muted = cx.theme().muted_foreground;
        let count = self.render_count(compact, cx);
        let notice = self.render_filter_notice();
        let filtered = self.filter.is_some();
        let lines: Vec<AnyElement> = lines
            .unwrap_or_default()
            .into_iter()
            .map(|row| self.render_row(row, cx))
            .collect();
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
                    .when(!compact, |header| {
                        header.child(div().flex_shrink_0().font_medium().child("Logs"))
                    })
                    .child(
                        div()
                            .id("log-summary")
                            .test_support()
                            .aria_label(summary.clone())
                            .tooltip({
                                let summary = summary.clone();
                                move |window, cx| Tooltip::new(summary.clone()).build(window, cx)
                            })
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(summary),
                    )
                    .children(count)
                    .when(filtered, |header| {
                        header.child(
                            div()
                                .w(px(if compact {
                                    COMPACT_CONTEXT_PICKER_WIDTH
                                } else {
                                    CONTEXT_PICKER_WIDTH
                                }))
                                .flex_shrink_0()
                                .child(
                                    Select::new(&self.controls.context)
                                        .id("log-context-picker")
                                        .small()
                                        .when(!compact, |select| select.title_prefix("Context: "))
                                        .accessibility_label("Lines around each match"),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w(px(if compact {
                                COMPACT_SEVERITY_PICKER_WIDTH
                            } else {
                                SEVERITY_PICKER_WIDTH
                            }))
                            .flex_shrink_0()
                            .child(
                                Select::new(&self.controls.severity)
                                    .id("log-severity-picker")
                                    .small()
                                    .accessibility_label("Severity"),
                            ),
                    )
                    .child(
                        div()
                            .key_context(LOG_FILTER_CONTEXT)
                            .w(px(if compact {
                                COMPACT_FILTER_WIDTH
                            } else {
                                FILTER_WIDTH
                            }))
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
                        Button::new("log-export")
                            .small()
                            .ghost()
                            .icon(IconName::Download)
                            .tooltip("Export lines")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ExportLines), cx);
                            }),
                    )
                    .child({
                        let mode = if following { "Pause" } else { "Follow" };
                        Button::new("log-mode")
                            .small()
                            .ghost()
                            .map(|button| {
                                if compact {
                                    button
                                        .icon(if following {
                                            IconName::Pause
                                        } else {
                                            IconName::Play
                                        })
                                        .tooltip(mode)
                                        .accessibility_label(mode)
                                } else {
                                    button.label(mode)
                                }
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_following(!following, cx);
                            }))
                    }),
            )
            .child(timeline)
            .map(|dock| match notice {
                Some(notice) => dock.child(notice).into_any_element(),
                None => dock
                    .child(
                        div()
                            .id("log-lines")
                            .track_focus(&self.lines_focus)
                            .key_context(LOG_LINES_CONTEXT)
                            .on_action(cx.listener(|this, _: &SelectNextLine, _, cx| {
                                this.move_selection(1, false, cx);
                            }))
                            .on_action(cx.listener(|this, _: &SelectPreviousLine, _, cx| {
                                this.move_selection(-1, false, cx);
                            }))
                            .on_action(cx.listener(|this, _: &ExtendNextLine, _, cx| {
                                this.move_selection(1, true, cx);
                            }))
                            .on_action(cx.listener(|this, _: &ExtendPreviousLine, _, cx| {
                                this.move_selection(-1, true, cx);
                            }))
                            .on_action(cx.listener(|this, _: &ClearLineSelection, window, cx| {
                                this.clear_selection(window, cx);
                            }))
                            .flex_1()
                            .min_h_0()
                            .px_3()
                            .pt_1()
                            .flex()
                            .flex_col()
                            .when(following, |lines| lines.justify_end())
                            .overflow_hidden()
                            .text_size(cx.theme().mono_font_size)
                            .font_family(cx.theme().mono_font_family.clone())
                            .on_scroll(cx.listener(|this, event, _, cx| {
                                this.scroll(event, cx);
                            }))
                            .children(lines),
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
        chosen_severity, clock_time, follow_summary, gap_text, line_summary, match_count,
        no_matches_title,
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
}
