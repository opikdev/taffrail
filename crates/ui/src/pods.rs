use std::cmp::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::table::{Column, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{LogTarget, PodKey, PodRow, PodSnapshot, log_targets};

/// The height of one table row, which `Size::Small` sets.
const ROW_HEIGHT: Pixels = px(30.);
const NAME: usize = 0;
const NAMESPACE: usize = 1;
const STATUS: usize = 2;
const NODE: usize = 3;
const READY: usize = 4;
const RESTARTS: usize = 5;
const AGE: usize = 6;
const COLUMNS: [(&str, f32); 7] = [
    ("Name", 320.),
    ("Namespace", 120.),
    ("Status", 170.),
    ("Node", 130.),
    ("Ready", 64.),
    ("Restarts", 80.),
    ("Age", 60.),
];
pub const ALWAYS_SHOWN_COLUMN: &str = COLUMNS[NAME].0;
const STATUS_DOT: f32 = 6.;
pub const ALL_NAMESPACES: &str = "All namespaces";
const CELL_PADDING: Edges<Pixels> = Edges {
    top: px(3.),
    bottom: px(3.),
    left: px(6.),
    right: px(6.),
};
const PANEL_EDGE_PADDING: Pixels = px(12.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Healthy,
    Degraded,
    Failed,
    Neutral,
}

pub fn status_tone(status: &str) -> Tone {
    match status {
        "Running" => Tone::Healthy,
        "Completed" | "Succeeded" => Tone::Neutral,
        "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => Tone::Degraded,
        _ => Tone::Failed,
    }
}

impl Tone {
    /// Lower sorts first, so that what needs attention comes first.
    fn urgency(self) -> u8 {
        match self {
            Tone::Failed => 0,
            Tone::Degraded => 1,
            Tone::Healthy => 2,
            Tone::Neutral => 3,
        }
    }

    pub fn color(self, cx: &App) -> Hsla {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Default)]
pub struct PodTable {
    snapshot: Arc<PodSnapshot>,
    filter: String,
    namespace: Option<String>,
    hidden: [bool; COLUMNS.len()],
    sort: Option<(usize, SortOrder)>,
    rows: Vec<Arc<PodRow>>,
    selected: Option<PodKey>,
    selected_row: Option<usize>,
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
        self.rebuild();
    }

    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    pub fn set_namespace(&mut self, namespace: Option<String>) {
        self.namespace = namespace;
        self.rebuild();
    }

    /// What the namespace picker offers. A chosen namespace stays listed after it disappears,
    /// so the picker can keep showing it.
    pub fn namespace_options(&self) -> Vec<&str> {
        let mut namespaces: Vec<&str> = namespaces(&self.snapshot).collect();
        if let Some(chosen) = self.namespace()
            && let Err(ix) = namespaces.binary_search(&chosen)
        {
            namespaces.insert(ix, chosen);
        }
        namespaces.insert(0, ALL_NAMESPACES);
        namespaces
    }

    /// The chosen namespace, if the cluster has no pods in it.
    pub fn empty_namespace(&self) -> Option<&str> {
        let namespace = self.namespace()?;
        let has_pods = self
            .snapshot
            .rows
            .iter()
            .any(|row| row.namespace == namespace);
        (!has_pods).then_some(namespace)
    }

    pub fn hidden_columns(&self) -> Vec<&'static str> {
        column_names()
            .zip(self.hidden)
            .filter_map(|(name, hidden)| hidden.then_some(name))
            .collect()
    }

    /// Hides the columns with these names and shows the rest. Hiding the sorted column
    /// returns to the default order.
    pub fn set_hidden_columns(&mut self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        self.hidden = Default::default();
        for name in names {
            let name = name.as_ref();
            let column = column_names().position(|column| column == name);
            if let Some(col_ix) = column.filter(|col_ix| *col_ix != NAME) {
                self.hidden[col_ix] = true;
            }
        }
        if self.sort.is_some_and(|(sorted, _)| self.hidden[sorted]) {
            self.sort = None;
            self.rebuild();
        }
    }

    /// The `COLUMNS` index shown at a table position, which hidden columns shift.
    fn column_at(&self, position: usize) -> usize {
        (0..COLUMNS.len())
            .filter(|col_ix| !self.hidden[*col_ix])
            .nth(position)
            .unwrap_or(NAME)
    }

    /// What a tail would follow: the rows as shown, so the pods on top come first.
    pub fn log_targets(&self) -> Vec<LogTarget> {
        log_targets(&self.rows)
    }

    /// Sorts by a column, then reverses it, then returns to the default order.
    fn cycle_sort(&mut self, col_ix: usize) {
        self.sort = match self.sort {
            Some((sorted, SortOrder::Ascending)) if sorted == col_ix => {
                Some((col_ix, SortOrder::Descending))
            }
            Some((sorted, SortOrder::Descending)) if sorted == col_ix => None,
            _ => Some((col_ix, SortOrder::Ascending)),
        };
        self.rebuild();
    }

    pub fn selected(&self) -> Option<&PodKey> {
        self.selected.as_ref()
    }

    /// Selects a pod by identity. Returns its row, or `None` if filters are hiding it.
    pub fn select_key(&mut self, key: PodKey) -> Option<usize> {
        self.selected = Some(key);
        self.rebuild();
        self.selected_row
    }

    pub fn selected_row(&self) -> Option<usize> {
        self.selected_row
    }

    pub fn namespaces(&self) -> Vec<&str> {
        namespaces(&self.snapshot).collect()
    }

    /// What a tail of the selection would follow.
    pub fn selection_targets(&self) -> Option<Vec<LogTarget>> {
        let key = self.selected.as_ref()?;
        Some(log_targets(self.snapshot.find(key)))
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_row = None;
    }

    fn row_key(&self, row_ix: usize) -> Option<PodKey> {
        self.rows.get(row_ix).map(|row| row.key())
    }

    fn select_row(&mut self, row_ix: usize) {
        self.selected = self.row_key(row_ix);
        self.selected_row = self.selected.is_some().then_some(row_ix);
    }

    /// Selects the row `delta` away from the selection, or the first row if there is none.
    /// Returns the row now selected.
    pub fn move_selection(&mut self, delta: isize) -> Option<usize> {
        let last = self.rows.len().checked_sub(1)?;
        let row_ix = match self.selected_row {
            Some(row_ix) => row_ix.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.select_row(row_ix);
        Some(row_ix)
    }

    fn rebuild(&mut self) {
        self.rows = self
            .snapshot
            .rows
            .iter()
            .filter(|row| {
                let in_namespace = self
                    .namespace()
                    .is_none_or(|namespace| row.namespace == namespace);
                in_namespace && row.matches(&self.filter)
            })
            .cloned()
            .collect();
        sort_rows(&mut self.rows, self.sort);
        self.selected_row = self.selected.as_ref().and_then(|selected| {
            let (namespace, name) = selected;
            self.rows
                .iter()
                .position(|row| row.namespace == *namespace && row.name == *name)
        });
    }

    fn cell(&self, row_ix: usize, col_ix: usize) -> SharedString {
        let Some(row) = self.rows.get(row_ix) else {
            return SharedString::default();
        };
        cell_text(row, col_ix, unix_now())
    }
}

pub fn column_names() -> impl Iterator<Item = &'static str> {
    COLUMNS.iter().map(|(name, _)| *name)
}

/// Each namespace once. Snapshot rows arrive sorted by namespace, so this needs no set.
fn namespaces(snapshot: &PodSnapshot) -> impl Iterator<Item = &str> {
    snapshot
        .rows
        .chunk_by(|a, b| a.namespace == b.namespace)
        .map(|rows| rows[0].namespace.as_str())
}

fn ready_counts(ready: &str) -> (u32, u32) {
    let (ready, total) = ready.split_once('/').unwrap_or_default();
    (ready.parse().unwrap_or(0), total.parse().unwrap_or(0))
}

pub fn urgency(row: &PodRow) -> u8 {
    status_tone(&row.status).urgency()
}

/// Snapshot rows arrive sorted by namespace and name, and these sorts are stable, so equal
/// rows always keep that order. With no column chosen, the pods that need attention lead.
fn sort_rows(rows: &mut [Arc<PodRow>], sort: Option<(usize, SortOrder)>) {
    let Some((col_ix, order)) = sort else {
        rows.sort_by_cached_key(|row| urgency(row));
        return;
    };
    let by = |ordering: Ordering| match order {
        SortOrder::Ascending => ordering,
        SortOrder::Descending => ordering.reverse(),
    };
    match col_ix {
        NAME => rows.sort_by(|a, b| by(a.name.cmp(&b.name))),
        NAMESPACE => rows.sort_by(|a, b| by(a.namespace.cmp(&b.namespace))),
        STATUS => {
            rows.sort_by(|a, b| by(urgency(a).cmp(&urgency(b)).then(a.status.cmp(&b.status))))
        }
        NODE => rows.sort_by(|a, b| by(a.node.cmp(&b.node))),
        READY => rows.sort_by(|a, b| by(ready_counts(&a.ready).cmp(&ready_counts(&b.ready)))),
        RESTARTS => rows.sort_by(|a, b| by(a.restarts.cmp(&b.restarts))),
        AGE => rows.sort_by(|a, b| by(b.created_unix.cmp(&a.created_unix))),
        _ => {}
    }
}

/// Where the list should scroll so `key` stays where it was on screen, after it moved
/// `from` one row `to` another. The top of the list stays put, so new rows there show.
fn anchored_offset(offset: Pixels, from: usize, to: usize) -> Pixels {
    if offset >= px(0.) {
        return offset;
    }
    offset - ROW_HEIGHT * (to as f32 - from as f32)
}

/// Shows `snapshot` while the row at the top of the view stays where the reader left it.
pub fn show_in_place(
    table: &mut TableState<PodTable>,
    snapshot: Arc<PodSnapshot>,
    cx: &mut Context<TableState<PodTable>>,
) {
    let handle = table.vertical_scroll_handle.0.borrow().base_handle.clone();
    let offset = handle.offset();
    let top = (-offset.y / ROW_HEIGHT).max(0.) as usize;
    let top_key = table.delegate().row_key(top);
    table.delegate_mut().show(snapshot);
    let moved_to = top_key.and_then(|(namespace, name)| {
        let rows = &table.delegate().rows;
        rows.iter()
            .position(|row| row.namespace == namespace && row.name == name)
    });
    if let Some(moved_to) = moved_to {
        handle.set_offset(point(offset.x, anchored_offset(offset.y, top, moved_to)));
    }
    cx.notify();
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

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

pub fn format_age(seconds: i64) -> String {
    match seconds.max(0) {
        seconds @ ..60 => format!("{seconds}s"),
        seconds @ ..3_600 => format!("{}m", seconds / 60),
        seconds @ ..86_400 => format!("{}h", seconds / 3_600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

impl TableDelegate for PodTable {
    fn columns_count(&self, _: &App) -> usize {
        self.hidden.iter().filter(|hidden| !**hidden).count()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, position: usize, _: &App) -> Column {
        let col_ix = self.column_at(position);
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

    fn render_th(
        &mut self,
        position: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col_ix = self.column_at(position);
        let (name, _) = COLUMNS[col_ix];
        let order = self
            .sort
            .filter(|(sorted, _)| *sorted == col_ix)
            .map(|(_, order)| order);
        h_flex()
            .id(("sort", col_ix))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Sort by {name}"))
            .size_full()
            .gap_1()
            .items_center()
            .when(!matches!(col_ix, NAME | NAMESPACE | STATUS | NODE), |th| {
                th.justify_end()
            })
            .child(name)
            .when_some(order, |th, order| {
                th.child(
                    Icon::new(match order {
                        SortOrder::Ascending => IconName::ArrowUp,
                        SortOrder::Descending => IconName::ArrowDown,
                    })
                    .size_3(),
                )
            })
            .on_click(cx.listener(move |table, _, _, cx| {
                table.delegate_mut().cycle_sort(col_ix);
                cx.notify();
            }))
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let selected = self.selected_row == Some(row_ix);
        div()
            .id(("row", row_ix))
            .when(selected, |row| row.bg(cx.theme().sidebar_accent))
            .on_click(cx.listener(move |table, _, _, cx| {
                table.delegate_mut().select_row(row_ix);
                cx.emit(TableEvent::SelectRow(row_ix));
                cx.notify();
            }))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        position: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col_ix = self.column_at(position);
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

    fn cell_label(&self, row_ix: usize, position: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row_ix, self.column_at(position)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::px;
    use taffrail_core::{PodRow, PodSnapshot};

    use super::{
        AGE, ALL_NAMESPACES, NAME, NODE, PodTable, READY, RESTARTS, ROW_HEIGHT, STATUS, SortOrder,
        Tone, anchored_offset, format_age, format_count, sort_rows, status_tone,
    };

    fn row(name: &str, status: &str, ready: &str, restarts: i32, created: i64) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: "shop".into(),
            name: name.into(),
            status: status.into(),
            ready: ready.into(),
            restarts,
            node: "node-1".into(),
            created_unix: Some(created),
            containers: vec!["app".into()],
        })
    }

    fn rows() -> Vec<Arc<PodRow>> {
        vec![
            row("a", "Running", "1/1", 0, 300),
            row("b", "CrashLoopBackOff", "0/1", 7, 100),
            row("c", "Pending", "0/2", 2, 200),
            row("d", "Completed", "2/2", 0, 400),
        ]
    }

    fn sorted(sort: Option<(usize, SortOrder)>) -> Vec<String> {
        let mut rows = rows();
        sort_rows(&mut rows, sort);
        rows.iter().map(|row| row.name.clone()).collect()
    }

    fn table(rows: Vec<Arc<PodRow>>) -> PodTable {
        let mut table = PodTable::default();
        table.show(Arc::new(PodSnapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn in_namespace(namespace: &str, name: &str) -> Arc<PodRow> {
        let mut row = Arc::unwrap_or_clone(row(name, "Running", "1/1", 0, 0));
        row.namespace = namespace.into();
        Arc::new(row)
    }

    fn across_namespaces() -> PodTable {
        table(vec![
            in_namespace("payments", "a"),
            in_namespace("payments", "b"),
            in_namespace("shop", "c"),
            in_namespace("web", "d"),
        ])
    }

    fn pod_names(table: &PodTable) -> Vec<&str> {
        table.rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn pods_that_need_attention_lead_by_default() {
        assert_eq!(sorted(None), ["b", "c", "a", "d"]);
    }

    #[test]
    fn numeric_columns_sort_as_numbers() {
        let mut rows = vec![
            row("a", "Running", "9/10", 10, 0),
            row("b", "Running", "10/10", 9, 0),
        ];
        sort_rows(&mut rows, Some((READY, SortOrder::Ascending)));
        assert_eq!(rows[0].name, "a");
        sort_rows(&mut rows, Some((RESTARTS, SortOrder::Ascending)));
        assert_eq!(rows[0].name, "b");
    }

    #[test]
    fn age_sorts_youngest_first_when_ascending() {
        assert_eq!(
            sorted(Some((AGE, SortOrder::Ascending))),
            ["d", "a", "c", "b"]
        );
        assert_eq!(
            sorted(Some((AGE, SortOrder::Descending))),
            ["b", "c", "a", "d"]
        );
    }

    #[test]
    fn descending_keeps_equal_rows_in_name_order() {
        let mut rows = vec![
            row("a", "Running", "1/1", 0, 0),
            row("b", "Running", "1/1", 0, 0),
        ];
        sort_rows(&mut rows, Some((STATUS, SortOrder::Descending)));
        assert_eq!(rows[0].name, "a");
    }

    #[test]
    fn clicking_a_header_cycles_up_down_and_back_to_the_default() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        assert_eq!(table.sort, Some((AGE, SortOrder::Ascending)));
        table.cycle_sort(AGE);
        assert_eq!(table.sort, Some((AGE, SortOrder::Descending)));
        table.cycle_sort(AGE);
        assert_eq!(table.sort, None);
    }

    #[test]
    fn a_new_sort_column_starts_ascending() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        table.cycle_sort(AGE);
        table.cycle_sort(READY);
        assert_eq!(table.sort, Some((READY, SortOrder::Ascending)));
    }

    #[test]
    fn a_tail_follows_the_rows_as_shown() {
        let targets = table(rows()).log_targets();
        let pods: Vec<_> = targets.iter().map(|target| target.pod.as_str()).collect();
        assert_eq!(pods, ["b", "c", "a", "d"]);
    }

    #[test]
    fn selection_follows_a_pod_when_its_row_moves() {
        let mut table = table(rows());
        table.select_row(2);
        assert_eq!(table.selected().unwrap().1, "a");

        table.show(Arc::new(PodSnapshot {
            rows: vec![
                row("a", "CrashLoopBackOff", "0/1", 1, 300),
                row("b", "CrashLoopBackOff", "0/1", 7, 100),
            ],
            synced: true,
            error: None,
        }));

        assert_eq!(table.selected_row, Some(0));
        assert_eq!(table.selected().unwrap().1, "a");
    }

    #[test]
    fn selection_survives_a_filter_that_hides_the_pod() {
        let mut table = table(rows());
        table.select_row(0);
        table.set_filter("zzz".into());
        assert_eq!(table.selected_row, None);
        assert_eq!(table.selected().unwrap().1, "b");

        table.set_filter(String::new());
        assert_eq!(table.selected_row, Some(0));
    }

    #[test]
    fn a_namespace_narrows_the_rows_and_the_tail() {
        let mut table = across_namespaces();
        table.set_namespace(Some("payments".into()));
        assert_eq!(pod_names(&table), ["a", "b"]);
        let targets = table.log_targets();
        assert!(targets.iter().all(|target| target.namespace == "payments"));
        assert_eq!(targets.len(), 2);

        table.set_namespace(None);
        assert_eq!(pod_names(&table), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_namespace_and_a_text_filter_narrow_together() {
        let mut table = across_namespaces();
        table.set_namespace(Some("payments".into()));
        table.set_filter("b".into());
        assert_eq!(pod_names(&table), ["b"]);
    }

    #[test]
    fn a_new_snapshot_stays_in_the_chosen_namespace() {
        let mut table = across_namespaces();
        table.set_namespace(Some("shop".into()));
        table.show(Arc::new(PodSnapshot {
            rows: vec![in_namespace("shop", "c"), in_namespace("web", "z")],
            synced: true,
            error: None,
        }));
        assert_eq!(pod_names(&table), ["c"]);
    }

    #[test]
    fn namespace_options_list_each_namespace_once_after_all() {
        assert_eq!(
            across_namespaces().namespace_options(),
            [ALL_NAMESPACES, "payments", "shop", "web"]
        );
    }

    #[test]
    fn namespace_options_are_only_all_for_an_empty_cluster() {
        assert_eq!(table(vec![]).namespace_options(), [ALL_NAMESPACES]);
    }

    #[test]
    fn a_namespace_that_disappears_stays_chosen_and_empty() {
        let mut table = across_namespaces();
        table.set_namespace(Some("shop".into()));
        assert_eq!(table.empty_namespace(), None);

        table.show(Arc::new(PodSnapshot {
            rows: vec![in_namespace("payments", "a"), in_namespace("web", "d")],
            synced: true,
            error: None,
        }));

        assert_eq!(table.namespace(), Some("shop"));
        assert_eq!(table.visible_rows(), 0);
        assert_eq!(table.empty_namespace(), Some("shop"));
        assert_eq!(
            table.namespace_options(),
            [ALL_NAMESPACES, "payments", "shop", "web"]
        );
    }

    #[test]
    fn a_namespace_hides_the_selection_without_losing_it() {
        let mut table = across_namespaces();
        table.select_row(3);
        table.set_namespace(Some("payments".into()));
        assert_eq!(table.selected_row, None);
        assert_eq!(table.selected().unwrap().1, "d");

        table.set_namespace(None);
        assert_eq!(table.selected_row, Some(3));
    }

    #[test]
    fn positions_skip_hidden_columns() {
        let mut table = table(rows());
        table.set_hidden_columns(["Namespace", "Ready"]);
        let shown: Vec<_> = (0..5).map(|position| table.column_at(position)).collect();
        assert_eq!(shown, [NAME, STATUS, NODE, RESTARTS, AGE]);
    }

    #[test]
    fn hidden_columns_read_back_in_table_order() {
        let mut table = table(rows());
        assert!(table.hidden_columns().is_empty());
        table.set_hidden_columns(vec![String::from("Age"), String::from("Node")]);
        assert_eq!(table.hidden_columns(), ["Node", "Age"]);
    }

    #[test]
    fn setting_hidden_columns_shows_the_others_again() {
        let mut table = table(rows());
        table.set_hidden_columns(["Age"]);
        table.set_hidden_columns(["Node"]);
        assert_eq!(table.hidden_columns(), ["Node"]);
    }

    #[test]
    fn the_name_column_cannot_be_hidden() {
        let mut table = table(rows());
        table.set_hidden_columns(["Name"]);
        assert!(table.hidden_columns().is_empty());
    }

    #[test]
    fn an_unknown_column_is_ignored() {
        let mut table = table(rows());
        table.set_hidden_columns(["Cost"]);
        assert!(table.hidden_columns().is_empty());
    }

    #[test]
    fn hiding_the_sorted_column_returns_to_the_default_order() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        assert_eq!(pod_names(&table), ["d", "a", "c", "b"]);

        table.set_hidden_columns(["Age"]);

        assert_eq!(table.sort, None);
        assert_eq!(pod_names(&table), ["b", "c", "a", "d"]);
    }

    #[test]
    fn hiding_another_column_keeps_the_sort() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        table.set_hidden_columns(["Node"]);
        assert_eq!(table.sort, Some((AGE, SortOrder::Ascending)));
    }

    #[test]
    fn moving_the_selection_stops_at_both_ends() {
        let mut table = table(rows());
        assert_eq!(table.move_selection(1), Some(0));
        assert_eq!(table.move_selection(-1), Some(0));
        assert_eq!(table.move_selection(10), Some(3));
        assert_eq!(table.move_selection(1), Some(3));
    }

    #[test]
    fn moving_the_selection_in_an_empty_table_does_nothing() {
        assert_eq!(table(vec![]).move_selection(1), None);
    }

    #[test]
    fn the_view_follows_a_row_that_moved_above_it() {
        assert_eq!(
            anchored_offset(px(-300.), 10, 12),
            px(-300.) - ROW_HEIGHT * 2.
        );
        assert_eq!(anchored_offset(px(-300.), 10, 9), px(-300.) + ROW_HEIGHT);
    }

    #[test]
    fn the_top_of_the_list_stays_put_so_new_rows_show() {
        assert_eq!(anchored_offset(px(0.), 0, 5), px(0.));
    }

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
