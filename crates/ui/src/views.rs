//! The resource views: which exist, and the live table and details panel behind each.
//!
//! To add a view, add a variant to [`View`] with its sidebar name, group, key and icon,
//! and a line to [`new_panel`]. The kind's table columns come from its [`TableRow`] and its
//! Summary tab from its [`Inspect`].

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::*;
use taffrail_core::{
    Backend, Connection, DaemonSetRow, DeploymentRow, Feed, Follow, JobRow, ObjectKey,
    ResourceWatch, StatefulSetRow, WatchError,
};

use crate::details::{DetailsEvent, DetailsPane, Inspect, PaneTab};
use crate::root::{Notice, Root, no_rows_notice};
use crate::table::{ResourceTable, SelectorFilter, column_names, show_in_place};

/// A resource view, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
}

impl View {
    pub const ALL: [View; 5] = [
        View::Pods,
        View::Deployments,
        View::StatefulSets,
        View::DaemonSets,
        View::Jobs,
    ];

    /// The sidebar label, which also names the view in saved state.
    pub fn name(self) -> &'static str {
        match self {
            View::Pods => "Pods",
            View::Deployments => "Deployments",
            View::StatefulSets => "StatefulSets",
            View::DaemonSets => "DaemonSets",
            View::Jobs => "Jobs",
        }
    }

    /// The sidebar group, which the panel header names before the view.
    pub fn group(self) -> &'static str {
        match self {
            View::Pods | View::Deployments | View::StatefulSets | View::DaemonSets | View::Jobs => {
                "Workloads"
            }
        }
    }

    /// The digit that opens the view. Each view keeps its own, so keys do not move as
    /// views are added: 1 Pods, 2 Deployments, 3 StatefulSets, 4 DaemonSets, 5 Jobs,
    /// 6 Nodes, 7 Namespaces, 8 Events.
    pub fn key(self) -> &'static str {
        match self {
            View::Pods => "1",
            View::Deployments => "2",
            View::StatefulSets => "3",
            View::DaemonSets => "4",
            View::Jobs => "5",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            View::Pods => IconName::Box,
            View::Deployments => IconName::Layers,
            View::StatefulSets => IconName::Database,
            View::DaemonSets => IconName::Boxes,
            View::Jobs => IconName::Play,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.name() == name)
    }

    /// The Kubernetes kind the view lists, such as `Deployment`.
    pub fn kind(self) -> &'static str {
        KIND_NAMES
            .iter()
            .find(|(_, name)| *name == self.name())
            .map_or(self.name(), |(kind, _)| kind)
    }

    /// The view that lists objects of a Kubernetes kind, such as `Deployment`, once one is
    /// registered. Links to a kind without a view show as plain text.
    pub fn for_kind(kind: &str) -> Option<Self> {
        KIND_NAMES
            .iter()
            .find(|(known, _)| *known == kind)
            .and_then(|(_, name)| Self::from_name(name))
    }
}

/// Each kind the sidebar can list, and the name of its view. A view registered under one of
/// these names opens links to its kind.
const KIND_NAMES: [(&str, &str); 13] = [
    ("Pod", "Pods"),
    ("Deployment", "Deployments"),
    ("StatefulSet", "StatefulSets"),
    ("DaemonSet", "DaemonSets"),
    ("Job", "Jobs"),
    ("CronJob", "CronJobs"),
    ("Service", "Services"),
    ("Ingress", "Ingresses"),
    ("ConfigMap", "ConfigMaps"),
    ("Secret", "Secrets"),
    ("Node", "Nodes"),
    ("Namespace", "Namespaces"),
    ("Event", "Events"),
];

/// Creates the panel behind a view other than Pods, which the root keeps apart.
pub fn new_panel(view: View, window: &mut Window, cx: &mut Context<Root>) -> Box<dyn Panel> {
    match view {
        View::Pods => Box::new(ResourcePanel::<taffrail_core::PodRow>::new(
            view, window, cx,
        )),
        View::Deployments => Box::new(ResourcePanel::<DeploymentRow>::new(view, window, cx)),
        View::StatefulSets => Box::new(ResourcePanel::<StatefulSetRow>::new(view, window, cx)),
        View::DaemonSets => Box::new(ResourcePanel::<DaemonSetRow>::new(view, window, cx)),
        View::Jobs => Box::new(ResourcePanel::<JobRow>::new(view, window, cx)),
    }
}

/// What the root needs to know about a view's rows to choose what to show.
pub struct PanelStatus {
    pub synced: bool,
    pub error: Option<WatchError>,
    pub total: usize,
    pub visible: usize,
    pub watching: bool,
}

/// A view's table, watch and details panel, without its kind of object.
pub trait Panel {
    fn view(&self) -> View;
    fn status(&self, cx: &App) -> PanelStatus;
    /// What to say when no rows show, though the watch has listed.
    fn empty_notice(&self, cx: &App) -> Notice;
    fn table(&self) -> AnyElement;
    /// Whether keyboard focus is on the table.
    fn table_focused(&self, window: &Window, cx: &App) -> bool;

    fn set_filter(&self, filter: String, cx: &mut App);
    fn set_selector(&self, selector: Option<SelectorFilter>, cx: &mut App);
    fn selector(&self, cx: &App) -> Option<SelectorFilter>;
    fn set_namespace(&self, namespace: Option<String>, cx: &mut App);
    fn namespaces(&self, cx: &App) -> Vec<String>;

    fn column_names(&self) -> Vec<&'static str>;
    fn hidden_columns(&self, cx: &App) -> Vec<&'static str>;
    fn set_hidden_columns(&self, names: &[String], cx: &mut App);

    /// Moves the selection and scrolls to it. Returns whether a row is selected.
    fn move_selection(&self, delta: isize, cx: &mut App) -> bool;
    /// Selects an object and scrolls to it. Returns whether its row shows.
    fn select(&self, key: ObjectKey, cx: &mut App) -> bool;
    /// Whether the watch has listed this object, whether or not its row shows.
    fn contains(&self, key: &ObjectKey, cx: &App) -> bool;
    /// The selected object, even while filters hide it.
    fn selected_key(&self, cx: &App) -> Option<ObjectKey>;
    fn has_selection(&self, cx: &App) -> bool;
    /// The pods a tail of the selected object follows, for a workload.
    fn selected_follow(&self, cx: &App) -> Option<Follow>;
    #[cfg(feature = "bench")]
    fn scroll_to_row(&self, row: usize, cx: &mut App);

    /// Starts watching, unless a watch is running.
    fn start(
        &mut self,
        backend: &Backend,
        connection: &Connection,
        namespace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    );
    /// Stops watching and forgets the rows.
    fn stop(&mut self, cx: &mut App);

    fn open_details(
        &mut self,
        backend: Arc<Backend>,
        connection: Connection,
        cx: &mut Context<Root>,
    );
    fn close_details(&mut self, cx: &mut App);
    fn details(&self) -> Option<AnyView>;
    fn show_tab(&self, tab: PaneTab, cx: &mut App);
    /// Moves the details panel to the newest row, or marks its object gone.
    fn update_details(&self, cx: &mut App);
}

pub struct ResourcePanel<R: Inspect> {
    view: View,
    pub(crate) table: Entity<TableState<ResourceTable<R>>>,
    watch: Option<ResourceWatch<R>>,
    task: Option<Task<()>>,
    details: Option<Entity<DetailsPane<R>>>,
    _table_events: Subscription,
    _details_events: Option<Subscription>,
}

impl<R: Inspect> ResourcePanel<R> {
    pub fn new(view: View, window: &mut Window, cx: &mut Context<Root>) -> Self {
        let table = cx.new(|cx| {
            TableState::new(ResourceTable::<R>::default(), window, cx)
                .col_movable(false)
                .col_resizable(false)
                .row_selectable(false)
        });
        let table_events =
            cx.subscribe_in(&table, window, |root, _, event: &TableEvent, window, cx| {
                if matches!(event, TableEvent::SelectRow(_)) {
                    root.focus(window, cx);
                    root.open_details(cx);
                }
            });
        Self {
            view,
            table,
            watch: None,
            task: None,
            details: None,
            _table_events: table_events,
            _details_events: None,
        }
    }

    /// The watch's snapshots, for following its objects without a watch of their own.
    pub fn feed(&self) -> Option<Feed<R>> {
        self.watch.as_ref().map(ResourceWatch::feed)
    }

    fn update_table(&self, cx: &mut App, update: impl FnOnce(&mut ResourceTable<R>)) {
        self.table.update(cx, |table, cx| {
            update(table.delegate_mut());
            cx.notify();
        });
    }

    fn scroll_to_selection(&self, cx: &mut App) {
        self.table.update(cx, |table, cx| {
            if let Some(row_ix) = table.delegate().selected_row() {
                table
                    .vertical_scroll_handle
                    .scroll_to_item(row_ix, ScrollStrategy::Nearest);
            }
            cx.notify();
        });
    }
}

impl<R: Inspect> Panel for ResourcePanel<R> {
    fn view(&self) -> View {
        self.view
    }

    fn status(&self, cx: &App) -> PanelStatus {
        let table = self.table.read(cx).delegate();
        let snapshot = table.snapshot();
        PanelStatus {
            synced: snapshot.synced,
            error: snapshot.error.clone(),
            total: snapshot.rows.len(),
            visible: table.visible_rows(),
            watching: self.watch.is_some(),
        }
    }

    fn empty_notice(&self, cx: &App) -> Notice {
        no_rows_notice(self.table.read(cx).delegate())
    }

    fn table(&self) -> AnyElement {
        DataTable::new(&self.table)
            .accessibility_label(self.view.name())
            .with_size(Size::Small)
            .bordered(false)
            .stripe(false)
            .into_any_element()
    }

    fn table_focused(&self, window: &Window, cx: &App) -> bool {
        self.table.focus_handle(cx).is_focused(window)
    }

    fn set_filter(&self, filter: String, cx: &mut App) {
        self.update_table(cx, |table| table.set_filter(filter));
    }

    fn set_selector(&self, selector: Option<SelectorFilter>, cx: &mut App) {
        self.update_table(cx, |table| table.set_selector(selector));
    }

    fn selector(&self, cx: &App) -> Option<SelectorFilter> {
        self.table.read(cx).delegate().selector().cloned()
    }

    fn set_namespace(&self, namespace: Option<String>, cx: &mut App) {
        if let Some(watch) = &self.watch {
            watch.set_namespace(namespace.clone());
        }
        self.update_table(cx, |table| table.set_namespace(namespace));
    }

    fn namespaces(&self, cx: &App) -> Vec<String> {
        let table = self.table.read(cx).delegate();
        table.namespaces().into_iter().map(str::to_owned).collect()
    }

    fn column_names(&self) -> Vec<&'static str> {
        column_names::<R>().collect()
    }

    fn hidden_columns(&self, cx: &App) -> Vec<&'static str> {
        self.table.read(cx).delegate().hidden_columns()
    }

    fn set_hidden_columns(&self, names: &[String], cx: &mut App) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_hidden_columns(names);
            table.refresh(cx);
            cx.notify();
        });
    }

    fn move_selection(&self, delta: isize, cx: &mut App) -> bool {
        self.table.update(cx, |table, cx| {
            let moved = table.delegate_mut().move_selection(delta);
            if let Some(row_ix) = moved {
                table
                    .vertical_scroll_handle
                    .scroll_to_item(row_ix, ScrollStrategy::Nearest);
            }
            cx.notify();
            moved.is_some()
        })
    }

    fn select(&self, key: ObjectKey, cx: &mut App) -> bool {
        let shown = self.table.update(cx, |table, _| {
            table.delegate_mut().select_key(key).is_some()
        });
        self.scroll_to_selection(cx);
        shown
    }

    fn contains(&self, key: &ObjectKey, cx: &App) -> bool {
        self.table
            .read(cx)
            .delegate()
            .snapshot()
            .find(key)
            .is_some()
    }

    fn selected_key(&self, cx: &App) -> Option<ObjectKey> {
        self.table.read(cx).delegate().selected().cloned()
    }

    fn has_selection(&self, cx: &App) -> bool {
        self.table.read(cx).delegate().selected().is_some()
    }

    fn selected_follow(&self, cx: &App) -> Option<Follow> {
        self.table
            .read(cx)
            .delegate()
            .selected_object()
            .and_then(|row| row.follow())
    }

    #[cfg(feature = "bench")]
    fn scroll_to_row(&self, row: usize, cx: &mut App) {
        self.table
            .update(cx, |table, cx| table.scroll_to_row(row, cx));
    }

    fn start(
        &mut self,
        backend: &Backend,
        connection: &Connection,
        namespace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) {
        if self.watch.is_some() {
            return;
        }
        let watch = backend.watch::<R>(connection, namespace);
        let mut feed = watch.feed();
        let table = self.table.clone();
        let view = self.view;
        self.watch = Some(watch);
        self.task = Some(cx.spawn_in(window, async move |root, cx| {
            while let Some(snapshot) = feed.next().await {
                let shown = root.update_in(cx, |root, window, cx| {
                    table.update(cx, |table, cx| show_in_place(table, snapshot, cx));
                    root.after_snapshot(view, window, cx);
                });
                if shown.is_err() {
                    return;
                }
            }
        }));
    }

    fn stop(&mut self, cx: &mut App) {
        self.task = None;
        self.watch = None;
        self.table.update(cx, |table, cx| {
            show_in_place(table, Arc::default(), cx);
        });
    }

    fn open_details(
        &mut self,
        backend: Arc<Backend>,
        connection: Connection,
        cx: &mut Context<Root>,
    ) {
        let Some(row) = self.table.read(cx).delegate().selected_object().cloned() else {
            return;
        };
        let details = cx.new(|cx| DetailsPane::new(backend, connection, row, cx));
        self._details_events = Some(cx.subscribe(&details, |root, _, event, cx| match event {
            DetailsEvent::Close => root.close_details(cx),
        }));
        self.details = Some(details);
    }

    fn close_details(&mut self, cx: &mut App) {
        self.details = None;
        self._details_events = None;
        self.update_table(cx, ResourceTable::clear_selection);
    }

    fn details(&self) -> Option<AnyView> {
        self.details.clone().map(AnyView::from)
    }

    fn show_tab(&self, tab: PaneTab, cx: &mut App) {
        if let Some(details) = &self.details {
            details.update(cx, |details, cx| details.show_tab(tab, cx));
        }
    }

    fn update_details(&self, cx: &mut App) {
        let Some(details) = self.details.clone() else {
            return;
        };
        let table = self.table.read(cx).delegate();
        if !table.snapshot().synced {
            return;
        }
        let row = table.selected_object().cloned();
        details.update(cx, |details, cx| match row {
            Some(row) => details.update_row(row, cx),
            None => details.mark_gone(cx),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{KIND_NAMES, View};
    use crate::sidebar::sidebar_names;

    #[test]
    fn each_view_has_its_own_name_and_key() {
        let mut names: Vec<_> = View::ALL.iter().map(|view| view.name()).collect();
        let mut keys: Vec<_> = View::ALL.iter().map(|view| view.key()).collect();
        names.sort_unstable();
        names.dedup();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(names.len(), View::ALL.len());
        assert_eq!(keys.len(), View::ALL.len());
        for view in View::ALL {
            assert_eq!(View::from_name(view.name()), Some(view));
        }
    }

    #[test]
    fn every_view_opens_links_to_its_kind() {
        for view in View::ALL {
            let kind = KIND_NAMES
                .iter()
                .find(|(_, name)| *name == view.name())
                .map(|(kind, _)| *kind);
            assert_eq!(kind, Some(view.kind()), "{} has no kind", view.name());
            assert_eq!(View::for_kind(view.kind()), Some(view));
        }
    }

    #[test]
    fn a_kind_without_a_view_opens_nothing() {
        assert_eq!(View::for_kind("ReplicaSet"), None);
        for (kind, name) in KIND_NAMES {
            assert_eq!(
                View::for_kind(kind).is_some(),
                View::from_name(name).is_some()
            );
        }
    }

    #[test]
    fn every_sidebar_item_has_a_kind() {
        let names: Vec<_> = KIND_NAMES.iter().map(|(_, name)| *name).collect();
        assert_eq!(names, sidebar_names().collect::<Vec<_>>());
    }
}
