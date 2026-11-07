use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{PodRow, PodSnapshot};

const NAME: usize = 0;
const NAMESPACE: usize = 1;
const STATUS: usize = 2;
const NODE: usize = 3;
const READY: usize = 4;
const RESTARTS: usize = 5;
const COLUMNS: [(&str, f32); 7] = [
    ("Name", 400.),
    ("Namespace", 140.),
    ("Status", 170.),
    ("Node", 130.),
    ("Ready", 64.),
    ("Restarts", 80.),
    ("Age", 60.),
];
const STATUS_DOT: f32 = 6.;
const CELL_PADDING: Edges<Pixels> = Edges {
    top: px(3.),
    bottom: px(3.),
    left: px(6.),
    right: px(6.),
};
const PANEL_EDGE_PADDING: Pixels = px(12.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Healthy,
    Degraded,
    Failed,
    Neutral,
}

fn status_tone(status: &str) -> Tone {
    match status {
        "Running" => Tone::Healthy,
        "Completed" | "Succeeded" => Tone::Neutral,
        "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => Tone::Degraded,
        _ => Tone::Failed,
    }
}

impl Tone {
    fn color(self, cx: &App) -> Hsla {
        match self {
            Tone::Healthy => cx.theme().success,
            Tone::Degraded => cx.theme().warning,
            Tone::Failed => cx.theme().danger,
            Tone::Neutral => cx.theme().muted_foreground,
        }
    }
}

fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[derive(Default)]
pub struct PodTable {
    snapshot: Arc<PodSnapshot>,
    filter: String,
    rows: Vec<Arc<PodRow>>,
}

impl PodTable {
    pub fn snapshot(&self) -> &PodSnapshot {
        &self.snapshot
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn show(&mut self, snapshot: Arc<PodSnapshot>) {
        self.snapshot = snapshot;
        self.apply_filter();
    }

    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        self.rows = self
            .snapshot
            .rows
            .iter()
            .filter(|row| row.matches(&self.filter))
            .cloned()
            .collect();
    }

    fn cell(&self, row_ix: usize, col_ix: usize) -> SharedString {
        let Some(row) = self.rows.get(row_ix) else {
            return SharedString::default();
        };
        cell_text(row, col_ix, unix_now())
    }
}

fn cell_text(row: &PodRow, col_ix: usize, now_unix: i64) -> SharedString {
    match col_ix {
        NAME => row.name.clone().into(),
        NAMESPACE => row.namespace.clone().into(),
        STATUS => row.status.clone().into(),
        READY => row.ready.clone().into(),
        RESTARTS => row.restarts.to_string().into(),
        NODE => row.node.clone().into(),
        _ => row
            .created_unix
            .map(|created| format_age(now_unix - created))
            .unwrap_or_default()
            .into(),
    }
}

/// Groups thousands, such as `5,500`.
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn format_age(seconds: i64) -> String {
    match seconds.max(0) {
        seconds @ ..60 => format!("{seconds}s"),
        seconds @ ..3_600 => format!("{}m", seconds / 60),
        seconds @ ..86_400 => format!("{}h", seconds / 3_600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

impl TableDelegate for PodTable {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (name, width) = COLUMNS[col_ix];
        let column = Column::new(name, name).width(px(width));
        match col_ix {
            NAME => column.paddings(Edges {
                left: PANEL_EDGE_PADDING,
                ..CELL_PADDING
            }),
            NAMESPACE | STATUS | NODE => column,
            _ => column.text_right(),
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.cell(row_ix, col_ix);
        let muted = cx.theme().muted_foreground;
        match col_ix {
            NAME => div().child(text),
            NAMESPACE | NODE => div().text_color(muted).child(text),
            STATUS => {
                let color = status_tone(&text).color(cx);
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_color(color)
                    .child(div().size(px(STATUS_DOT)).rounded_full().bg(color))
                    .child(text)
            }
            _ => div().font_features(tabular_figures()).child(text),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row_ix, col_ix))
    }
}

#[cfg(test)]
mod tests {
    use super::{Tone, format_age, format_count, status_tone};

    #[test]
    fn counts_group_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(5_500), "5,500");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn status_tone_flags_anything_unknown_as_failed() {
        assert_eq!(status_tone("Running"), Tone::Healthy);
        assert_eq!(status_tone("Completed"), Tone::Neutral);
        assert_eq!(status_tone("Terminating"), Tone::Degraded);
        assert_eq!(status_tone("CrashLoopBackOff"), Tone::Failed);
        assert_eq!(status_tone("Init:Error"), Tone::Failed);
    }

    #[test]
    fn age_uses_the_largest_whole_unit() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3_599), "59m");
        assert_eq!(format_age(3_600), "1h");
        assert_eq!(format_age(86_399), "23h");
        assert_eq!(format_age(86_400), "1d");
    }

    #[test]
    fn age_treats_a_future_time_as_zero() {
        assert_eq!(format_age(-5), "0s");
    }
}
