//! Counts lines over time by severity, for the timeline above the log lines.
//!
//! Counts are kept per store segment, so dropping a segment drops its counts without a
//! rescan. Each part keeps at most `MAX_BINS` time bins, and widens its bins when it would
//! keep more, so memory stays bounded whatever timestamps the lines carry.

use std::collections::{BTreeMap, VecDeque};

use super::matches::MatchSet;
use super::parse::{Severity, split_timestamp};
use super::store::Gap;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// Bin widths in seconds. Each divides the next, so counts in narrow bins add up exactly
/// into wider ones.
const WIDTHS: [i64; 22] = [
    1,
    5,
    10,
    30,
    MINUTE,
    5 * MINUTE,
    10 * MINUTE,
    30 * MINUTE,
    HOUR,
    3 * HOUR,
    6 * HOUR,
    12 * HOUR,
    DAY,
    2 * DAY,
    4 * DAY,
    8 * DAY,
    16 * DAY,
    32 * DAY,
    64 * DAY,
    128 * DAY,
    256 * DAY,
    512 * DAY,
];
/// The most bins one part keeps before it widens them.
const MAX_BINS: usize = 1024;

/// The groups the timeline stacks, from the bottom of a bar up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Info,
    /// Debug lines, and lines with no severity.
    Other,
}

impl Level {
    pub const ALL: [Self; 4] = [Self::Error, Self::Warning, Self::Info, Self::Other];

    fn of(severity: Severity) -> Self {
        match severity {
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
            Severity::Info => Self::Info,
            Severity::Debug | Severity::Unknown => Self::Other,
        }
    }

    /// The word the timeline's legend shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warning => "WARN",
            Self::Info => "INFO",
            Self::Other => "OTHER",
        }
    }
}

/// Lines counted in each [`Level`], indexed in the order of `Level::ALL`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelCounts(pub [u64; 4]);

impl LevelCounts {
    pub fn get(&self, level: Level) -> u64 {
        self.0[level as usize]
    }

    pub fn total(&self) -> u64 {
        self.0.iter().sum()
    }

    fn add(&mut self, other: &Self) {
        for (count, other) in self.0.iter_mut().zip(other.0) {
            *count += other;
        }
    }
}

/// What a stored line adds to the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    /// A line, which only the error index counts when it has no timestamp.
    Line {
        stamp: Option<i64>,
        severity: Severity,
    },
    Gap(Gap),
}

impl Mark {
    /// What a stored record adds, from its header and text.
    pub(super) fn of(gap: bool, severity: Severity, text: &[u8]) -> Option<Self> {
        if gap {
            return Gap::parse(text).map(Self::Gap);
        }
        Some(Self::Line {
            stamp: split_timestamp(text).0,
            severity,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Bin {
    counts: LevelCounts,
    /// The first line stored in this bin.
    first: u64,
}

#[derive(Clone, Copy, Debug)]
struct GapMark {
    seq: u64,
    gap: Gap,
}

/// The counts of one store segment.
struct Part {
    first_seq: u64,
    /// An index into `WIDTHS`.
    step: usize,
    /// Keyed by the bin's start, in whole widths since the Unix epoch.
    bins: BTreeMap<i64, Bin>,
    gaps: Vec<GapMark>,
}

impl Part {
    fn new(first_seq: u64) -> Self {
        Self {
            first_seq,
            step: 0,
            bins: BTreeMap::new(),
            gaps: Vec::new(),
        }
    }

    fn width(&self) -> i64 {
        WIDTHS[self.step]
    }

    fn add(&mut self, seq: u64, mark: Mark) {
        match mark {
            Mark::Line {
                stamp: Some(stamp),
                severity,
            } => {
                let key = seconds(stamp).div_euclid(self.width());
                let bin = self.bins.entry(key).or_insert(Bin {
                    counts: LevelCounts::default(),
                    first: seq,
                });
                bin.counts.0[Level::of(severity) as usize] += 1;
                bin.first = bin.first.min(seq);
                if self.bins.len() > MAX_BINS {
                    self.widen();
                }
            }
            Mark::Line { stamp: None, .. } => {}
            Mark::Gap(gap) => self.gaps.push(GapMark { seq, gap }),
        }
    }

    /// Moves to the next width that keeps the bins within `MAX_BINS`.
    fn widen(&mut self) {
        while self.bins.len() > MAX_BINS && self.step + 1 < WIDTHS.len() {
            let (old, new) = (self.width(), WIDTHS[self.step + 1]);
            self.step += 1;
            let mut wider = BTreeMap::new();
            for (key, bin) in std::mem::take(&mut self.bins) {
                let wide = (key * old).div_euclid(new);
                merge(&mut wider, wide, &bin);
            }
            self.bins = wider;
        }
    }
}

fn merge(bins: &mut BTreeMap<i64, Bin>, key: i64, bin: &Bin) {
    bins.entry(key)
        .and_modify(|merged| {
            merged.counts.add(&bin.counts);
            merged.first = merged.first.min(bin.first);
        })
        .or_insert(*bin);
}

fn ceil_div(value: i64, by: i64) -> i64 {
    -(-value).div_euclid(by)
}

fn seconds(nanos: i64) -> i64 {
    nanos.div_euclid(NANOS_PER_SECOND)
}

/// Line counts over time, kept up to date as lines are stored and dropped.
#[derive(Default)]
pub(super) struct Timeline {
    parts: VecDeque<Part>,
    /// The error lines counted, so the next one can be found without a scan.
    errors: MatchSet,
}

impl Timeline {
    /// Counts line `seq`, stored in the segment that starts at `part`.
    pub(super) fn add(&mut self, part: u64, seq: u64, mark: Mark) {
        if let Mark::Line {
            severity: Severity::Error,
            ..
        } = mark
        {
            self.errors.insert(seq);
        }
        let at = match self.parts.back() {
            Some(last) if last.first_seq == part => self.parts.len() - 1,
            Some(last) if last.first_seq < part => {
                self.parts.push_back(Part::new(part));
                self.parts.len() - 1
            }
            None => {
                self.parts.push_back(Part::new(part));
                0
            }
            Some(_) => match self
                .parts
                .binary_search_by_key(&part, |part| part.first_seq)
            {
                Ok(at) => at,
                Err(at) => {
                    self.parts.insert(at, Part::new(part));
                    at
                }
            },
        };
        self.parts[at].add(seq, mark);
    }

    /// Forgets the lines below `seq`, which starts a segment.
    pub(super) fn remove_below(&mut self, seq: u64) {
        while self.parts.front().is_some_and(|part| part.first_seq < seq) {
            self.parts.pop_front();
        }
        self.errors.remove_below(seq);
    }

    /// The first error line at or after `seq`.
    pub(super) fn next_error(&self, seq: u64) -> Option<u64> {
        let rank = self.errors.rank(seq);
        self.errors.select(rank..rank + 1).first().copied()
    }

    /// The last error line before `seq`.
    pub(super) fn previous_error(&self, seq: u64) -> Option<u64> {
        let rank = self.errors.rank(seq).checked_sub(1)?;
        self.errors.select(rank..rank + 1).first().copied()
    }

    /// The counts in at most `max_buckets` buckets of equal width, over the time the lines
    /// and gaps span. The width is the narrowest of `WIDTHS` that fits.
    pub(super) fn view(&self, max_buckets: usize) -> TimelineView {
        let max_buckets = max_buckets.max(1) as i64;
        let mut span: Option<(i64, i64)> = None;
        let mut widen = |from: i64, to: i64| {
            span = Some(span.map_or((from, to), |(start, end)| (start.min(from), end.max(to))));
        };
        let mut step = 0;
        for part in &self.parts {
            step = step.max(part.step);
            let width = part.width();
            if let (Some((first, _)), Some((last, _))) =
                (part.bins.first_key_value(), part.bins.last_key_value())
            {
                widen(first * width, (last + 1) * width);
            }
            for mark in &part.gaps {
                let (from, to) = (
                    mark.gap.from.min(mark.gap.to),
                    mark.gap.to.max(mark.gap.from),
                );
                widen(seconds(from), seconds(to) + 1);
            }
        }
        let Some((start, end)) = span else {
            return TimelineView::default();
        };
        let fits = |width: i64| ceil_div(end, width) - start.div_euclid(width) <= max_buckets;
        let step = (step..WIDTHS.len())
            .find(|step| fits(WIDTHS[*step]))
            .unwrap_or(WIDTHS.len() - 1);
        let width = WIDTHS[step];
        let first = start.div_euclid(width);
        let count = (ceil_div(end, width) - first) as usize;
        let mut buckets = vec![Bucket::default(); count];
        for part in &self.parts {
            for (key, bin) in &part.bins {
                let at = ((key * part.width()).div_euclid(width) - first) as usize;
                let bucket = &mut buckets[at];
                bucket.counts.add(&bin.counts);
                bucket.first = Some(bucket.first.map_or(bin.first, |seq| seq.min(bin.first)));
            }
            for mark in &part.gaps {
                let (from, to) = (
                    mark.gap.from.min(mark.gap.to),
                    mark.gap.to.max(mark.gap.from),
                );
                let from = (seconds(from).div_euclid(width) - first) as usize;
                let to = (seconds(to).div_euclid(width) - first) as usize;
                for bucket in &mut buckets[from..=to.min(count - 1)] {
                    bucket.gap = true;
                    bucket.first = Some(bucket.first.map_or(mark.seq, |seq| seq.min(mark.seq)));
                }
            }
        }
        TimelineView {
            start: first * width * NANOS_PER_SECOND,
            width: width * NANOS_PER_SECOND,
            buckets,
        }
    }

    /// Roughly how much memory the counts take.
    pub(super) fn bytes(&self) -> usize {
        self.parts
            .iter()
            .map(|part| {
                part.bins.len() * (size_of::<i64>() + size_of::<Bin>() + 16)
                    + part.gaps.capacity() * size_of::<GapMark>()
            })
            .sum::<usize>()
            + self.errors.bytes()
    }
}

/// One bar of the timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bucket {
    pub counts: LevelCounts,
    /// The first line stored in this time, or the first gap row, to jump to.
    pub first: Option<u64>,
    /// Whether lines may be missing in this time.
    pub gap: bool,
}

/// Lines over time, in buckets of equal width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineView {
    /// When the first bucket starts, in nanoseconds since the Unix epoch.
    pub start: i64,
    /// How long each bucket lasts, in nanoseconds.
    pub width: i64,
    pub buckets: Vec<Bucket>,
}

impl TimelineView {
    /// When bucket `index` starts.
    pub fn bucket_start(&self, index: usize) -> i64 {
        self.start + index as i64 * self.width
    }

    /// The bucket that holds the time `nanos`, if any does.
    pub fn bucket_at(&self, nanos: i64) -> Option<usize> {
        if self.width == 0 || nanos < self.start {
            return None;
        }
        let index = ((nanos - self.start) / self.width) as usize;
        (index < self.buckets.len()).then_some(index)
    }

    /// The lines counted across every bucket.
    pub fn totals(&self) -> LevelCounts {
        let mut totals = LevelCounts::default();
        for bucket in &self.buckets {
            totals.add(&bucket.counts);
        }
        totals
    }

    /// Whether any bucket marks lines that may be missing.
    pub fn has_gaps(&self) -> bool {
        self.buckets.iter().any(|bucket| bucket.gap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10:42:00 UTC on 7 November 2026, a whole minute.
    const T0: i64 = 1_794_048_120 * NANOS_PER_SECOND;

    fn at(seconds: i64) -> i64 {
        T0 + seconds * NANOS_PER_SECOND
    }

    fn line(seconds: i64, severity: Severity) -> Mark {
        Mark::Line {
            stamp: Some(at(seconds)),
            severity,
        }
    }

    fn counts(error: u64, warning: u64, info: u64, other: u64) -> LevelCounts {
        LevelCounts([error, warning, info, other])
    }

    #[test]
    fn counts_lines_per_second_by_level() {
        let mut timeline = Timeline::default();
        timeline.add(0, 0, line(0, Severity::Info));
        timeline.add(0, 1, line(0, Severity::Error));
        timeline.add(0, 2, line(2, Severity::Debug));
        timeline.add(0, 3, line(2, Severity::Unknown));
        timeline.add(0, 4, line(2, Severity::Warning));

        let view = timeline.view(100);

        assert_eq!(view.start, at(0));
        assert_eq!(view.width, NANOS_PER_SECOND);
        assert_eq!(
            view.buckets,
            [
                Bucket {
                    counts: counts(1, 0, 1, 0),
                    first: Some(0),
                    gap: false,
                },
                Bucket::default(),
                Bucket {
                    counts: counts(0, 1, 0, 2),
                    first: Some(2),
                    gap: false,
                },
            ]
        );
        assert_eq!(view.totals(), counts(1, 1, 1, 2));
    }

    #[test]
    fn buckets_widen_to_fit_the_span() {
        let mut timeline = Timeline::default();
        for second in 0..600 {
            timeline.add(0, second as u64, line(second, Severity::Info));
        }

        let view = timeline.view(100);

        assert_eq!(view.width, 10 * NANOS_PER_SECOND);
        assert_eq!(view.buckets.len(), 60);
        assert!(
            view.buckets
                .iter()
                .all(|bucket| bucket.counts == counts(0, 0, 10, 0))
        );
        assert_eq!(view.buckets[3].first, Some(30));
        assert_eq!(view.bucket_at(at(35)), Some(3));
        assert_eq!(view.bucket_at(at(-1)), None);
        assert_eq!(view.bucket_at(at(600)), None);
        assert_eq!(view.bucket_start(3), at(30));
    }

    #[test]
    fn the_first_line_of_a_bucket_is_the_first_stored() {
        let mut timeline = Timeline::default();
        timeline.add(0, 0, line(5, Severity::Info));
        timeline.add(0, 1, line(1, Severity::Info));
        timeline.add(100, 100, line(1, Severity::Info));

        let view = timeline.view(1);

        assert_eq!(view.buckets.len(), 1);
        assert_eq!(view.buckets[0].first, Some(0));
        assert_eq!(timeline.view(10).buckets[0].first, Some(1));
    }

    #[test]
    fn dropping_a_segment_drops_its_counts() {
        let mut timeline = Timeline::default();
        timeline.add(0, 0, line(0, Severity::Error));
        timeline.add(0, 1, line(1, Severity::Info));
        timeline.add(2, 2, line(1, Severity::Error));
        timeline.add(2, 3, line(9, Severity::Info));

        timeline.remove_below(2);

        let view = timeline.view(100);
        assert_eq!(view.start, at(1));
        assert_eq!(view.totals(), counts(1, 0, 1, 0));
        assert_eq!(view.buckets[0].first, Some(2));
        assert_eq!(timeline.next_error(0), Some(2));
        assert_eq!(timeline.previous_error(2), None);
    }

    #[test]
    fn parts_added_out_of_order_stay_in_order() {
        let mut timeline = Timeline::default();
        timeline.add(200, 250, line(3, Severity::Info));
        timeline.add(0, 10, line(1, Severity::Info));
        timeline.add(100, 150, line(2, Severity::Info));

        timeline.remove_below(100);

        assert_eq!(timeline.view(100).totals(), counts(0, 0, 2, 0));
    }

    #[test]
    fn finds_the_errors_around_a_line() {
        let mut timeline = Timeline::default();
        for seq in 0..10 {
            let severity = if seq % 4 == 1 {
                Severity::Error
            } else {
                Severity::Info
            };
            timeline.add(0, seq, line(seq as i64, severity));
        }

        assert_eq!(timeline.next_error(0), Some(1));
        assert_eq!(timeline.next_error(1), Some(1));
        assert_eq!(timeline.next_error(2), Some(5));
        assert_eq!(timeline.next_error(10), None);
        assert_eq!(timeline.previous_error(5), Some(1));
        assert_eq!(timeline.previous_error(6), Some(5));
        assert_eq!(timeline.previous_error(1), None);
    }

    #[test]
    fn a_gap_marks_every_bucket_it_covers() {
        let mut timeline = Timeline::default();
        timeline.add(0, 0, line(0, Severity::Info));
        timeline.add(
            0,
            1,
            Mark::Gap(Gap {
                from: at(2),
                to: at(4) + 500_000_000,
            }),
        );
        timeline.add(0, 2, line(6, Severity::Info));

        let view = timeline.view(100);

        let gaps: Vec<bool> = view.buckets.iter().map(|bucket| bucket.gap).collect();
        assert_eq!(gaps, [false, false, true, true, true, false, false]);
        assert_eq!(view.buckets[3].first, Some(1));
        assert_eq!(view.buckets[3].counts.total(), 0);
        assert!(view.has_gaps());
    }

    #[test]
    fn a_gap_alone_still_shows() {
        let mut timeline = Timeline::default();
        timeline.add(
            0,
            0,
            Mark::Gap(Gap {
                from: at(0),
                to: at(30),
            }),
        );

        let view = timeline.view(10);

        assert_eq!(view.width, 5 * NANOS_PER_SECOND);
        assert!(view.buckets.iter().all(|bucket| bucket.gap));
    }

    #[test]
    fn an_empty_timeline_has_no_buckets() {
        assert_eq!(Timeline::default().view(100), TimelineView::default());
        assert_eq!(TimelineView::default().bucket_at(T0), None);
    }

    #[test]
    fn widely_spread_times_keep_memory_bounded() {
        let mut timeline = Timeline::default();
        for seq in 0..20_000u64 {
            let second = (seq * 7_919) as i64 % (30 * DAY);
            timeline.add(0, seq, line(second, Severity::Info));
        }

        let part = &timeline.parts[0];
        assert!(part.bins.len() <= MAX_BINS);
        assert!(part.width() >= HOUR);
        let view = timeline.view(200);
        assert_eq!(view.totals().total(), 20_000);
        assert!(view.buckets.len() <= 200);
        assert!(timeline.bytes() < 64 * 1024);
    }

    #[test]
    fn times_before_the_epoch_land_in_their_own_buckets() {
        let mut timeline = Timeline::default();
        timeline.add(
            0,
            0,
            Mark::Line {
                stamp: Some(-1),
                severity: Severity::Info,
            },
        );
        timeline.add(
            0,
            1,
            Mark::Line {
                stamp: Some(0),
                severity: Severity::Info,
            },
        );

        let view = timeline.view(10);

        assert_eq!(view.start, -NANOS_PER_SECOND);
        assert_eq!(view.buckets.len(), 2);
    }

    /// Run with `cargo test --release -p taffrail-core timeline_speed -- --ignored --nocapture`.
    ///
    /// Counts as many lines as a full store holds, at 10,000 lines per second across 16
    /// segments, then times building the bars the dock shows.
    #[test]
    #[ignore = "a measurement, run in release"]
    fn timeline_speed() {
        use std::time::Instant;

        const LINES: u64 = 8_400_000;
        const PER_SEGMENT: u64 = LINES / 16;
        const SEVERITIES: [Severity; 5] = [
            Severity::Info,
            Severity::Info,
            Severity::Debug,
            Severity::Warning,
            Severity::Error,
        ];
        let marks: Vec<Mark> = (0..LINES)
            .map(|seq| Mark::Line {
                stamp: Some(T0 + (seq * 100_000) as i64),
                severity: SEVERITIES[(seq % 5) as usize],
            })
            .collect();
        let mut timeline = Timeline::default();
        let started = Instant::now();
        for (seq, mark) in marks.into_iter().enumerate() {
            let seq = seq as u64;
            timeline.add(seq / PER_SEGMENT * PER_SEGMENT, seq, mark);
        }
        let add = started.elapsed();
        let started = Instant::now();
        let runs = 100;
        let mut view = TimelineView::default();
        for _ in 0..runs {
            view = timeline.view(240);
        }
        let build = started.elapsed() / runs;
        println!(
            "{LINES} lines: {:.1} ns per line counted, {:.0} KiB kept, {} bars of {} s built in {:.0} us",
            add.as_nanos() as f64 / LINES as f64,
            timeline.bytes() as f64 / 1024.,
            view.buckets.len(),
            view.width / NANOS_PER_SECOND,
            build.as_secs_f64() * 1e6,
        );
    }
}
