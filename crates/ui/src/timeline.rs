//! The log timeline: bars over time, stacked by severity, above the log lines.

use chrono::{DateTime, TimeZone};
use gpui_kit::component::Theme;
use gpui_kit::*;
use taffrail_core::{Level, LevelCounts, TimelineView};

use crate::pods::format_count;

/// The strip's height, with its bars and the row of times and legend below them.
pub const TIMELINE_HEIGHT: f32 = 46.;
pub const BARS_HEIGHT: f32 = 26.;
/// The room each bar takes, including the space after it.
const BAR_PITCH: f32 = 6.;
const MIN_BUCKETS: usize = 8;
const MAX_BUCKETS: usize = 400;
/// The shortest a severity's part of a bar draws, so a single error stays visible.
const MIN_SEGMENT: f32 = 2.;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
const MINUTE: i64 = 60 * NANOS_PER_SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// The key context of the focused timeline, where arrow keys move between bars.
pub const LOG_TIMELINE_CONTEXT: &str = "LogTimeline";

actions!(taffrail, [TimelinePrevious, TimelineNext]);

/// How many bars fit in a strip this wide.
pub fn max_buckets(width: Pixels) -> usize {
    ((f32::from(width) / BAR_PITCH) as usize).clamp(MIN_BUCKETS, MAX_BUCKETS)
}

/// The height of each severity's part of a bar, in the order of `Level::ALL`.
///
/// The bar's height follows its line count against the tallest bar. Each severity it
/// holds draws at least `MIN_SEGMENT` high, and the rest shares out by count.
pub fn segment_heights(counts: &LevelCounts, max: u64, area: f32) -> [f32; 4] {
    let total = counts.total();
    if total == 0 || max == 0 {
        return [0.; 4];
    }
    let present = counts.0.iter().filter(|count| **count > 0).count() as f32;
    let height = (total as f32 / max as f32 * area)
        .max(present * MIN_SEGMENT)
        .min(area);
    let spare = (height - present * MIN_SEGMENT).max(0.);
    counts.0.map(|count| {
        if count == 0 {
            0.
        } else {
            MIN_SEGMENT + spare * count as f32 / total as f32
        }
    })
}

/// The status colour of a severity's bars. Other lines stay muted, like their word.
pub fn level_color(level: Level, theme: &Theme) -> Hsla {
    match level {
        Level::Error => theme.danger,
        Level::Warning => theme.warning,
        Level::Info => theme.info,
        Level::Other => theme.muted_foreground.opacity(0.55),
    }
}

/// How long a bar lasts, such as `10 s` or `2 days`.
pub fn duration_label(nanos: i64) -> String {
    let (value, unit) = if nanos >= DAY {
        (nanos / DAY, "day")
    } else if nanos >= HOUR {
        (nanos / HOUR, "h")
    } else if nanos >= MINUTE {
        (nanos / MINUTE, "min")
    } else {
        (nanos / NANOS_PER_SECOND, "s")
    };
    match (value, unit) {
        (1, "day") => "1 day".to_owned(),
        (value, "day") => format!("{value} days"),
        (value, unit) => format!("{value} {unit}"),
    }
}

/// A time on the strip, as precise as its bars: seconds for bars under a minute, the
/// date for bars of a day or more, and the date too when the strip spans days.
pub fn axis_time(
    nanos: i64,
    view: &TimelineView,
    zone: &impl TimeZone<Offset: std::fmt::Display>,
) -> String {
    let span = view.width * view.buckets.len() as i64;
    let format = if view.width >= DAY {
        "%b %-d"
    } else if span > DAY {
        "%b %-d %H:%M"
    } else if view.width >= MINUTE {
        "%H:%M"
    } else {
        "%H:%M:%S"
    };
    DateTime::from_timestamp_nanos(nanos)
        .with_timezone(zone)
        .format(format)
        .to_string()
}

/// What a bar holds, for its tooltip and screen readers.
pub fn bucket_label(
    view: &TimelineView,
    index: usize,
    zone: &impl TimeZone<Offset: std::fmt::Display>,
) -> String {
    let bucket = &view.buckets[index];
    let start = view.bucket_start(index);
    let range = format!(
        "{} to {}",
        axis_time(start, view, zone),
        axis_time(start + view.width, view, zone)
    );
    let mut parts: Vec<String> = Level::ALL
        .iter()
        .filter(|level| bucket.counts.get(**level) > 0)
        .map(|level| {
            format!(
                "{} {}",
                format_count(bucket.counts.get(*level) as usize),
                level.label()
            )
        })
        .collect();
    if parts.is_empty() {
        parts.push("no lines".to_owned());
    }
    if bucket.gap {
        parts.push("lines may be missing".to_owned());
    }
    format!("{range}: {}", parts.join(", "))
}

/// Whether a bar has anything to jump to.
fn has_lines(view: &TimelineView, index: usize) -> bool {
    view.buckets[index].first.is_some()
}

/// The next bar with lines after `from`, or before it when going back. With no bar to
/// start from, forward starts before the first bar and back after the last.
pub fn next_bucket(view: &TimelineView, from: Option<usize>, forward: bool) -> Option<usize> {
    let count = view.buckets.len();
    if forward {
        let start = from.map_or(0, |from| from + 1);
        (start..count).find(|index| has_lines(view, *index))
    } else {
        let end = from.unwrap_or(count).min(count);
        (0..end).rev().find(|index| has_lines(view, *index))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use taffrail_core::{Bucket, LevelCounts, TimelineView};

    use super::{
        DAY, MINUTE, NANOS_PER_SECOND, axis_time, bucket_label, duration_label, max_buckets,
        next_bucket, segment_heights,
    };

    /// 10:42:00 UTC on 7 November 2026.
    const T0: i64 = 1_794_048_120 * NANOS_PER_SECOND;

    fn bucket(counts: [u64; 4], first: Option<u64>, gap: bool) -> Bucket {
        Bucket {
            counts: LevelCounts(counts),
            first,
            gap,
        }
    }

    fn view(width: i64, buckets: Vec<Bucket>) -> TimelineView {
        TimelineView {
            start: T0,
            width,
            buckets,
        }
    }

    #[test]
    fn bars_fit_the_strip() {
        assert_eq!(max_buckets(gpui_kit::px(600.)), 100);
        assert_eq!(max_buckets(gpui_kit::px(10.)), 8);
        assert_eq!(max_buckets(gpui_kit::px(9_000.)), 400);
    }

    #[test]
    fn a_bar_is_as_tall_as_its_share_of_the_tallest() {
        let heights = segment_heights(&LevelCounts([0, 0, 50, 50]), 100, 24.);
        assert_eq!(heights, [0., 0., 12., 12.]);
        assert_eq!(
            segment_heights(&LevelCounts([0, 0, 100, 0]), 100, 24.),
            [0., 0., 24., 0.]
        );
        assert_eq!(segment_heights(&LevelCounts::default(), 100, 24.), [0.; 4]);
    }

    #[test]
    fn a_single_error_stays_visible_in_a_tall_bar() {
        let heights = segment_heights(&LevelCounts([1, 0, 999, 0]), 1_000, 24.);

        assert!(heights[0] >= 2.);
        assert!((heights.iter().sum::<f32>() - 24.).abs() < 0.01);
    }

    #[test]
    fn a_short_bar_still_shows_each_severity() {
        let heights = segment_heights(&LevelCounts([1, 1, 1, 1]), 10_000, 24.);

        assert_eq!(heights, [2.; 4]);
    }

    #[test]
    fn durations_name_their_unit() {
        assert_eq!(duration_label(NANOS_PER_SECOND), "1 s");
        assert_eq!(duration_label(30 * NANOS_PER_SECOND), "30 s");
        assert_eq!(duration_label(5 * MINUTE), "5 min");
        assert_eq!(duration_label(3 * 60 * MINUTE), "3 h");
        assert_eq!(duration_label(DAY), "1 day");
        assert_eq!(duration_label(4 * DAY), "4 days");
    }

    #[test]
    fn times_are_as_precise_as_the_bars() {
        let seconds = view(NANOS_PER_SECOND, vec![Bucket::default(); 10]);
        assert_eq!(
            axis_time(T0 + 5 * NANOS_PER_SECOND, &seconds, &Utc),
            "10:42:05"
        );
        let minutes = view(MINUTE, vec![Bucket::default(); 10]);
        assert_eq!(axis_time(T0, &minutes, &Utc), "10:42");
        let days_of_hours = view(60 * MINUTE, vec![Bucket::default(); 30]);
        assert_eq!(axis_time(T0, &days_of_hours, &Utc), "Nov 7 10:42");
        let days = view(DAY, vec![Bucket::default(); 3]);
        assert_eq!(axis_time(T0, &days, &Utc), "Nov 7");
    }

    #[test]
    fn a_bar_names_its_time_and_counts_in_words() {
        let timeline = view(
            10 * NANOS_PER_SECOND,
            vec![
                bucket([3, 0, 1_204, 2], Some(0), false),
                bucket([0; 4], Some(9), true),
                bucket([0; 4], None, false),
            ],
        );

        assert_eq!(
            bucket_label(&timeline, 0, &Utc),
            "10:42:00 to 10:42:10: 3 ERROR, 1,204 INFO, 2 OTHER"
        );
        assert_eq!(
            bucket_label(&timeline, 1, &Utc),
            "10:42:10 to 10:42:20: no lines, lines may be missing"
        );
        assert_eq!(
            bucket_label(&timeline, 2, &Utc),
            "10:42:20 to 10:42:30: no lines"
        );
    }

    #[test]
    fn arrows_skip_bars_with_nothing_to_show() {
        let timeline = view(
            NANOS_PER_SECOND,
            vec![
                bucket([0, 0, 1, 0], Some(0), false),
                bucket([0; 4], None, false),
                bucket([0; 4], Some(4), true),
                bucket([0; 4], None, false),
            ],
        );

        assert_eq!(next_bucket(&timeline, Some(0), true), Some(2));
        assert_eq!(next_bucket(&timeline, Some(2), true), None);
        assert_eq!(next_bucket(&timeline, Some(2), false), Some(0));
        assert_eq!(next_bucket(&timeline, Some(0), false), None);
        assert_eq!(next_bucket(&timeline, None, true), Some(0));
        assert_eq!(next_bucket(&timeline, None, false), Some(2));
        assert_eq!(next_bucket(&TimelineView::default(), None, false), None);
    }
}
