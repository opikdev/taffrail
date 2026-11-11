//! The table every resource view shows: its rows, sorting, filters, hidden columns, and a
//! selection that belongs to an object rather than to a position.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::table::{Column, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Follow, ObjectKey, ObjectLink, PodRow, Row, Selector, Snapshot};

/// The height of one table row, which `Size::Small` sets.
pub const ROW_HEIGHT: Pixels = px(30.);
pub const ALL_NAMESPACES: &str = "All namespaces";
const STATUS_DOT: f32 = 6.;
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

impl Tone {
    /// Lower sorts first, so that what needs attention comes first.
    pub fn urgency(self) -> u8 {
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

/// How a column draws its cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellStyle {
    Text,
    Muted,
    /// A dot and a word in the row's tone.
    Status,
    /// Right-aligned, with tabular figures.
    Number,
    /// The pods the row holds, from [`TableRow::pod_count`], as a number. The column's
    /// `text` and `compare` go unused.
    PodCount,
}

/// One column of a resource table.
pub struct ColumnDef<R> {
    pub name: &'static str,
    pub width: f32,
    pub style: CellStyle,
    /// The cell's text, given the time now in Unix seconds for ages.
    pub text: fn(&R, i64) -> SharedString,
    /// The ascending order. Equal rows keep the snapshot's namespace and name order.
    pub compare: fn(&R, &R) -> Ordering,
}

/// A kind of object a resource table can show.
pub trait TableRow: Row {
    /// What a notice calls one and many, such as `pod` and `pods`.
    const SINGULAR: &'static str;
    const PLURAL: &'static str;
    /// The columns, in order. The first is the name, which always shows.
    const COLUMNS: &'static [ColumnDef<Self>];

    /// How urgent the row looks: its status colour, and the default order.
    fn tone(&self) -> Tone;

    /// Labels, for a kind that a label selector can narrow.
    fn labels(&self) -> Option<&BTreeMap<String, String>> {
        None
    }

    /// The pods a workload runs, for a kind that runs pods.
    fn follow(&self) -> Option<Follow> {
        None
    }

    /// The pods **Show its pods** narrows the pod view to, and the namespace to show them
    /// in, or every namespace with `None`. A workload's pods are those it follows.
    fn pods(&self) -> Option<(SelectorFilter, Option<String>)> {
        let follow = self.follow()?;
        let filter = SelectorFilter {
            label: follow.label,
            selector: RowMatch::Labels(follow.selector),
        };
        Some((filter, follow.namespace))
    }

    /// A field a [`RowMatch::Field`] can narrow by, such as a pod's `spec.nodeName`.
    fn field(&self, _name: &str) -> Option<&str> {
        None
    }

    /// How many pods this row holds, for a kind with a [`CellStyle::PodCount`] column.
    fn pod_count(&self, _counts: &PodCounts) -> Option<usize> {
        None
    }

    /// Where `Enter` on the selected row goes, for a kind whose rows lead elsewhere. Other
    /// kinds open their details.
    fn jump(&self) -> Option<Jump> {
        None
    }

    /// Orders rows of equal urgency in the default order, lower first, such as newer events
    /// before older ones. Rows that tie keep the snapshot's namespace and name order.
    fn default_rank(&self) -> i64 {
        0
    }
}

/// Where `Enter` on a selected row goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Jump {
    /// Chooses this namespace, and returns to the view before.
    Namespace(String),
    /// Follows a link to another object, such as an event's pod, as its details would.
    Link(ObjectLink),
}

impl Jump {
    /// What the palette and the details button call it.
    pub fn label(&self) -> String {
        match self {
            Self::Namespace(namespace) => format!("Use namespace {namespace}"),
            Self::Link(link) => format!("Go to {}", link.label()),
        }
    }
}

/// Pods by the node they run on and by namespace, counted from the pod watch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodCounts {
    pub by_node: HashMap<String, usize>,
    pub by_namespace: HashMap<String, usize>,
}

impl PodCounts {
    pub fn new(pods: &[Arc<PodRow>]) -> Self {
        let mut counts = Self::default();
        for pod in pods {
            if !pod.node.is_empty() {
                *counts.by_node.entry(pod.node.clone()).or_default() += 1;
            }
            *counts
                .by_namespace
                .entry(pod.namespace.clone())
                .or_default() += 1;
        }
        counts
    }
}

pub fn urgency<R: TableRow>(row: &R) -> u8 {
    row.tone().urgency()
}

pub fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Rows narrowed to those a label selector or a field matches, with what to call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorFilter {
    /// Such as `deployment/checkout-api` or `node/node-07`.
    pub label: String,
    pub selector: RowMatch,
}

/// What a [`SelectorFilter`] matches rows by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowMatch {
    /// A label selector, for a workload's pods.
    Labels(Selector),
    /// A field that equals a value, such as a pod's `spec.nodeName`.
    Field { name: &'static str, value: String },
}

impl RowMatch {
    pub fn matches<R: TableRow>(&self, row: &R) -> bool {
        match self {
            Self::Labels(selector) => row.labels().is_some_and(|labels| selector.matches(labels)),
            Self::Field { name, value } => row.field(name) == Some(value.as_str()),
        }
    }
}

pub struct ResourceTable<R> {
    pub(crate) snapshot: Arc<Snapshot<R>>,
    filter: String,
    selector: Option<SelectorFilter>,
    namespace: Option<String>,
    hidden: Vec<bool>,
    pub(crate) sort: Option<(usize, SortOrder)>,
    pub(crate) rows: Vec<Arc<R>>,
    selected: Option<ObjectKey>,
    pub(crate) selected_row: Option<usize>,
    pod_counts: Arc<PodCounts>,
}

impl<R: TableRow> Default for ResourceTable<R> {
    fn default() -> Self {
        Self {
            snapshot: Arc::default(),
            filter: String::new(),
            selector: None,
            namespace: None,
            hidden: vec![false; R::COLUMNS.len()],
            sort: None,
            rows: Vec::new(),
            selected: None,
            selected_row: None,
            pod_counts: Arc::default(),
        }
    }
}

impl<R: TableRow> ResourceTable<R> {
    pub fn snapshot(&self) -> &Snapshot<R> {
        &self.snapshot
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn show(&mut self, snapshot: Arc<Snapshot<R>>) {
        self.snapshot = snapshot;
        self.rebuild();
    }

    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }

    /// The rows as shown, after filters and sorting.
    pub fn rows(&self) -> &[Arc<R>] {
        &self.rows
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn selector(&self) -> Option<&SelectorFilter> {
        self.selector.as_ref()
    }

    pub fn set_selector(&mut self, selector: Option<SelectorFilter>) {
        self.selector = selector;
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

    /// The chosen namespace, if the cluster has nothing of this kind in it.
    pub fn empty_namespace(&self) -> Option<&str> {
        let namespace = self.namespace().filter(|_| R::NAMESPACED)?;
        let has_rows = self
            .snapshot
            .rows
            .iter()
            .any(|row| row.namespace() == namespace);
        (!has_rows).then_some(namespace)
    }

    /// Whether a column counts pods, so the table wants [`PodCounts`].
    pub fn counts_pods() -> bool {
        R::COLUMNS
            .iter()
            .any(|column| column.style == CellStyle::PodCount)
    }

    pub fn set_pod_counts(&mut self, counts: Arc<PodCounts>) {
        self.pod_counts = counts;
        let sorted = self.sort.map(|(col_ix, _)| R::COLUMNS[col_ix].style);
        if sorted == Some(CellStyle::PodCount) {
            self.rebuild();
        }
    }

    fn pod_count(&self, row: &R) -> Option<usize> {
        row.pod_count(&self.pod_counts)
    }

    pub fn hidden_columns(&self) -> Vec<&'static str> {
        column_names::<R>()
            .zip(&self.hidden)
            .filter_map(|(name, hidden)| hidden.then_some(name))
            .collect()
    }

    /// Hides the columns with these names and shows the rest. Hiding the sorted column
    /// returns to the default order.
    pub fn set_hidden_columns(&mut self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        self.hidden = vec![false; R::COLUMNS.len()];
        for name in names {
            let name = name.as_ref();
            let column = column_names::<R>().position(|column| column == name);
            if let Some(col_ix) = column.filter(|col_ix| *col_ix != 0) {
                self.hidden[col_ix] = true;
            }
        }
        if self.sort.is_some_and(|(sorted, _)| self.hidden[sorted]) {
            self.sort = None;
            self.rebuild();
        }
    }

    /// The `COLUMNS` index shown at a table position, which hidden columns shift.
    pub(crate) fn column_at(&self, position: usize) -> usize {
        (0..R::COLUMNS.len())
            .filter(|col_ix| !self.hidden[*col_ix])
            .nth(position)
            .unwrap_or(0)
    }

    /// Sorts by a column, then reverses it, then returns to the default order.
    pub(crate) fn cycle_sort(&mut self, col_ix: usize) {
        self.sort = match self.sort {
            Some((sorted, SortOrder::Ascending)) if sorted == col_ix => {
                Some((col_ix, SortOrder::Descending))
            }
            Some((sorted, SortOrder::Descending)) if sorted == col_ix => None,
            _ => Some((col_ix, SortOrder::Ascending)),
        };
        self.rebuild();
    }

    pub fn selected(&self) -> Option<&ObjectKey> {
        self.selected.as_ref()
    }

    /// The selected object's row, even while filters hide it.
    pub fn selected_object(&self) -> Option<&Arc<R>> {
        self.snapshot.find(self.selected.as_ref()?)
    }

    /// Selects an object by identity. Returns its row, or `None` if filters are hiding it.
    pub fn select_key(&mut self, key: ObjectKey) -> Option<usize> {
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

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_row = None;
    }

    pub(crate) fn row_key(&self, row_ix: usize) -> Option<ObjectKey> {
        self.rows.get(row_ix).map(|row| row.key())
    }

    pub(crate) fn select_row(&mut self, row_ix: usize) {
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
        let selector = self.selector.as_ref().map(|filter| &filter.selector);
        let filter = self.filter.to_lowercase();
        self.rows = self
            .snapshot
            .rows
            .iter()
            .filter(|row| {
                let in_namespace = !R::NAMESPACED
                    || self
                        .namespace()
                        .is_none_or(|namespace| row.namespace() == namespace);
                let selected = selector.is_none_or(|selector| selector.matches(row.as_ref()));
                in_namespace && selected && row.matches(&filter)
            })
            .cloned()
            .collect();
        match self.sort {
            Some((col_ix, order)) if R::COLUMNS[col_ix].style == CellStyle::PodCount => {
                let mut rows = std::mem::take(&mut self.rows);
                match order {
                    SortOrder::Ascending => rows.sort_by_cached_key(|row| self.pod_count(row)),
                    SortOrder::Descending => {
                        rows.sort_by_cached_key(|row| std::cmp::Reverse(self.pod_count(row)));
                    }
                }
                self.rows = rows;
            }
            sort => sort_rows(&mut self.rows, sort),
        }
        self.selected_row = self.selected.as_ref().and_then(|(namespace, name)| {
            self.rows
                .iter()
                .position(|row| row.namespace() == namespace && row.name() == name)
        });
    }

    fn cell(&self, row_ix: usize, col_ix: usize) -> SharedString {
        let Some(row) = self.rows.get(row_ix) else {
            return SharedString::default();
        };
        let column = &R::COLUMNS[col_ix];
        match column.style {
            CellStyle::PodCount => self
                .pod_count(row)
                .map(|count| format_count(count).into())
                .unwrap_or_default(),
            _ => (column.text)(row, unix_now()),
        }
    }
}

pub fn column_names<R: TableRow>() -> impl Iterator<Item = &'static str> {
    R::COLUMNS.iter().map(|column| column.name)
}

/// Each namespace once. Snapshot rows arrive sorted by namespace, so this needs no set.
fn namespaces<R: TableRow>(snapshot: &Snapshot<R>) -> impl Iterator<Item = &str> {
    snapshot
        .rows
        .chunk_by(|a, b| a.namespace() == b.namespace())
        .map(|rows| rows[0].namespace())
}

/// Snapshot rows arrive sorted by namespace and name, and these sorts are stable, so equal
/// rows always keep that order. With no column chosen, the rows that need attention lead.
pub fn sort_rows<R: TableRow>(rows: &mut [Arc<R>], sort: Option<(usize, SortOrder)>) {
    let Some((col_ix, order)) = sort else {
        rows.sort_by_cached_key(|row| (urgency(row.as_ref()), row.default_rank()));
        return;
    };
    let Some(column) = R::COLUMNS.get(col_ix) else {
        return;
    };
    let compare = column.compare;
    match order {
        SortOrder::Ascending => rows.sort_by(|a, b| compare(a, b)),
        SortOrder::Descending => rows.sort_by(|a, b| compare(a, b).reverse()),
    }
}

/// Where the list should scroll so `key` stays where it was on screen, after it moved
/// `from` one row `to` another. The top of the list stays put, so new rows there show.
pub fn anchored_offset(offset: Pixels, from: usize, to: usize) -> Pixels {
    if offset >= px(0.) {
        return offset;
    }
    offset - ROW_HEIGHT * (to as f32 - from as f32)
}

/// Shows `snapshot` while the row at the top of the view stays where the reader left it.
pub fn show_in_place<R: TableRow>(
    table: &mut TableState<ResourceTable<R>>,
    snapshot: Arc<Snapshot<R>>,
    cx: &mut Context<TableState<ResourceTable<R>>>,
) {
    let handle = table.vertical_scroll_handle.0.borrow().base_handle.clone();
    let offset = handle.offset();
    let top = (-offset.y / ROW_HEIGHT).max(0.) as usize;
    let top_key = table.delegate().row_key(top);
    table.delegate_mut().show(snapshot);
    let moved_to = top_key.and_then(|(namespace, name)| {
        let rows = &table.delegate().rows;
        rows.iter()
            .position(|row| row.namespace() == namespace && row.name() == name)
    });
    if let Some(moved_to) = moved_to {
        handle.set_offset(point(offset.x, anchored_offset(offset.y, top, moved_to)));
    }
    cx.notify();
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

/// An age cell, blank without a creation time.
pub fn age_text(created_unix: Option<i64>, now_unix: i64) -> SharedString {
    created_unix
        .map(|created| format_age(now_unix - created))
        .unwrap_or_default()
        .into()
}

/// Youngest first when ascending.
pub fn by_age(a: Option<i64>, b: Option<i64>) -> Ordering {
    b.cmp(&a)
}

impl<R: TableRow> TableDelegate for ResourceTable<R> {
    fn columns_count(&self, _: &App) -> usize {
        self.hidden.iter().filter(|hidden| !**hidden).count()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, position: usize, _: &App) -> Column {
        let col_ix = self.column_at(position);
        let ColumnDef {
            name, width, style, ..
        } = R::COLUMNS[col_ix];
        let column = Column::new(name, name).width(px(width));
        match (col_ix, style) {
            (0, _) => column.paddings(Edges {
                left: PANEL_EDGE_PADDING,
                ..CELL_PADDING
            }),
            (_, CellStyle::Number | CellStyle::PodCount) => column.text_right(),
            _ => column,
        }
    }

    fn render_th(
        &mut self,
        position: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col_ix = self.column_at(position);
        let ColumnDef { name, style, .. } = R::COLUMNS[col_ix];
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
            .when(
                matches!(style, CellStyle::Number | CellStyle::PodCount),
                |th| th.justify_end(),
            )
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
        match R::COLUMNS[col_ix].style {
            CellStyle::Text => div().child(text),
            CellStyle::Muted => div().text_color(muted).child(text),
            CellStyle::Status => {
                let tone = self
                    .rows
                    .get(row_ix)
                    .map_or(Tone::Neutral, |row| row.tone());
                let color = tone.color(cx);
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_color(color)
                    .child(div().size(px(STATUS_DOT)).rounded_full().bg(color))
                    .child(text)
            }
            CellStyle::Number | CellStyle::PodCount => {
                div().font_features(tabular_figures()).child(text)
            }
        }
    }

    fn cell_label(&self, row_ix: usize, position: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row_ix, self.column_at(position)))
    }
}
