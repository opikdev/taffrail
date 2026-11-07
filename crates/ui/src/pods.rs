use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::table::{Column, TableDelegate, TableState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{PodRow, PodSnapshot};

const COLUMNS: [(&str, f32); 7] = [
    ("Namespace", 160.),
    ("Name", 320.),
    ("Status", 150.),
    ("Ready", 70.),
    ("Restarts", 80.),
    ("Node", 160.),
    ("Age", 70.),
];

#[derive(Default)]
pub struct PodTable {
    snapshot: Arc<PodSnapshot>,
}

impl PodTable {
    pub fn show(&mut self, snapshot: Arc<PodSnapshot>) {
        self.snapshot = snapshot;
    }

    fn cell(&self, row_ix: usize, col_ix: usize) -> SharedString {
        let Some(row) = self.snapshot.rows.get(row_ix) else {
            return SharedString::default();
        };
        cell_text(row, col_ix, unix_now())
    }
}

fn cell_text(row: &PodRow, col_ix: usize, now_unix: i64) -> SharedString {
    match col_ix {
        0 => row.namespace.clone().into(),
        1 => row.name.clone().into(),
        2 => row.status.clone().into(),
        3 => row.ready.clone().into(),
        4 => row.restarts.to_string().into(),
        5 => row.node.clone().into(),
        _ => row
            .created_unix
            .map(|created| format_age(now_unix - created))
            .unwrap_or_default()
            .into(),
    }
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
        self.snapshot.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (name, width) = COLUMNS[col_ix];
        Column::new(name, name).width(px(width))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().child(self.cell(row_ix, col_ix))
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row_ix, col_ix))
    }
}

#[cfg(test)]
mod tests {
    use super::format_age;

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
