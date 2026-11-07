use std::ops::Range;
use std::time::Duration;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{LogLine, LogProgress, LogReader, LogTail};

const LINE_HEIGHT: f32 = 18.;
const SOURCE_WIDTH: f32 = 220.;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

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
            _updates: updates,
        }
    }

    fn held(&self) -> Range<u64> {
        self.progress.first..self.progress.end
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
            .targets()
            .get(line.source as usize)
            .map(|target| format!("{}/{}", target.pod, target.container))
            .unwrap_or_default()
    }

    fn render_line(&self, line: LogLine, cx: &App) -> impl IntoElement {
        let source = self.source_label(&line);
        div()
            .id(("log-line", line.seq))
            .test_support()
            .aria_label(format!("{source} {}", line.text))
            .h(px(LINE_HEIGHT))
            .flex()
            .gap_2()
            .flex_shrink_0()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .w(px(SOURCE_WIDTH))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .text_color(cx.theme().muted_foreground)
                    .child(source),
            )
            .child(div().child(line.text))
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
        let rows = (window.viewport_size().height / 2. / px(LINE_HEIGHT)) as u64;
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
        let summary = error.unwrap_or_else(|| {
            format!(
                "{} streams, {} lines kept",
                self.reader.targets().len(),
                held.end - held.start
            )
        });
        let following = self.following;
        div()
            .id("logs")
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("log-mode")
                            .small()
                            .label(if following { "Pause" } else { "Follow" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_following(!following, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("log-summary")
                            .test_support()
                            .aria_label(summary.clone())
                            .child(summary),
                    ),
            )
            .child(
                div()
                    .id("log-lines")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(following, |lines| lines.justify_end())
                    .overflow_hidden()
                    .text_xs()
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
    use super::{last_top, scrolled_top};

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
