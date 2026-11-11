use std::io;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::actions::Cancel;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::{Command, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, StyledExt, Theme, TitleBar, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{
    Backend, ClusterError, Connection, Contexts, Follow, LogFilter, LogTail, LogTarget,
    MAX_TAIL_PODS, ObjectKey, ObjectLink, PodKey, PodRow, PodSnapshot, SavedState, Selector,
    WatchError,
};

use crate::about::render_about;
use crate::details::{DETAILS_WIDTH, PaneTab};
use crate::logs::{
    BAR_HEIGHT, CONTEXT_CHOICES, ClearLineSelection, ExtendNextLine, ExtendPreviousLine,
    LOG_FILTER_CONTEXT, LOG_LINES_CONTEXT, LineContextPicker, LogControls, LogPane, ResetLogFilter,
    SEVERITY_CHOICES, SelectNextLine, SelectPreviousLine, SeverityPicker, chosen_context,
    chosen_severity, dock_height,
};
use crate::palette::{
    ClearSelector, CloseWindow, CopyLines, ExportLines, FocusFilter, FocusLogFilter,
    FollowSelected, GoBack, NextError, OpenLink, OpenPalette, OpenPod, OpenSelected, PALETTE_KEY,
    PaletteSource, PreviousError, ReloadContexts, ShowAbout, ShowEvents, ShowPods, ShowShortcuts,
    ShowSummary, ShowView, ShowYaml, SortBy, SwitchContext, SwitchNamespace, TailSelection,
    TailSelector, TailWorkload, ToggleFollow, ToggleSidebar, groups, render_shortcuts,
};
use crate::sidebar::{SidebarItem, resource_sidebar};
use crate::table::{ALL_NAMESPACES, Jump, PodCounts, ResourceTable, SelectorFilter, TableRow};
use crate::timeline::{LOG_TIMELINE_CONTEXT, TimelineNext, TimelinePrevious};
use crate::views::{Panel, ResourcePanel, View, new_core_events_panel, new_panel};
use crate::windows;

type ContextPicker = SelectState<Vec<String>>;
type NamespacePicker = SelectState<SearchableVec<String>>;

actions!(
    taffrail,
    [
        NextRow,
        PreviousRow,
        FirstRow,
        LastRow,
        NextPage,
        PreviousPage,
        FocusNextControl,
        FocusPreviousControl,
        CloseDetails,
        ClearFilter,
        ClearLogFilter,
        CloseOverlay
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
struct ToggleColumn {
    name: &'static str,
}

const KEY_CONTEXT: &str = "Taffrail";
const TABLE_KEYS: &str = "Taffrail && !Input";
const FILTER_CONTEXT: &str = "PodFilter";
/// The key context of GPUI Kit's table.
const TABLE_CONTEXT: &str = "DataTable";
const OPEN_PALETTE_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-k"
} else {
    "ctrl-k"
};
const TOGGLE_SIDEBAR_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-b"
} else {
    "ctrl-b"
};
const COPY_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-c"
} else {
    "ctrl-c"
};
const BACK_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-["
} else {
    "alt-left"
};
const CLOSE_WINDOW_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-w"
} else {
    "ctrl-w"
};
const SEARCH_HEIGHT: f32 = 26.;
const PALETTE_WIDTH: f32 = 560.;
const PALETTE_TOP: f32 = 96.;
const PALETTE_HEIGHT: f32 = 380.;
const SHORTCUTS_WIDTH: f32 = 420.;
const ABOUT_WIDTH: f32 = 320.;

/// Binds the keys that act on the pod table when no text field has focus.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", NextRow, Some(TABLE_KEYS)),
        KeyBinding::new("down", NextRow, Some(TABLE_KEYS)),
        KeyBinding::new("k", PreviousRow, Some(TABLE_KEYS)),
        KeyBinding::new("up", PreviousRow, Some(TABLE_KEYS)),
        KeyBinding::new("home", FirstRow, Some(TABLE_KEYS)),
        KeyBinding::new("end", LastRow, Some(TABLE_KEYS)),
        KeyBinding::new("pagedown", NextPage, Some(TABLE_KEYS)),
        KeyBinding::new("pageup", PreviousPage, Some(TABLE_KEYS)),
        KeyBinding::new("escape", CloseDetails, Some(TABLE_KEYS)),
        KeyBinding::new("enter", OpenSelected, Some(TABLE_KEYS)),
        KeyBinding::new("/", FocusFilter, Some(TABLE_KEYS)),
        KeyBinding::new("f", FocusLogFilter, Some(TABLE_KEYS)),
        KeyBinding::new("l", TailSelection, Some(TABLE_KEYS)),
        KeyBinding::new("w", TailWorkload, Some(TABLE_KEYS)),
        KeyBinding::new("p", ShowPods, Some(TABLE_KEYS)),
        KeyBinding::new(BACK_KEY, GoBack, Some(TABLE_KEYS)),
        KeyBinding::new("s", ShowSummary, Some(TABLE_KEYS)),
        KeyBinding::new("e", ShowEvents, Some(TABLE_KEYS)),
        KeyBinding::new("y", ShowYaml, Some(TABLE_KEYS)),
        KeyBinding::new("space", ToggleFollow, Some(TABLE_KEYS)),
        KeyBinding::new("n", NextError, Some(TABLE_KEYS)),
        KeyBinding::new("shift-n", PreviousError, Some(TABLE_KEYS)),
        KeyBinding::new("left", TimelinePrevious, Some(LOG_TIMELINE_CONTEXT)),
        KeyBinding::new("right", TimelineNext, Some(LOG_TIMELINE_CONTEXT)),
        KeyBinding::new("down", SelectNextLine, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new("up", SelectPreviousLine, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new("shift-down", ExtendNextLine, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new("shift-up", ExtendPreviousLine, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new("escape", ClearLineSelection, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new(COPY_KEY, CopyLines, Some(LOG_LINES_CONTEXT)),
        KeyBinding::new("?", ShowShortcuts, Some(TABLE_KEYS)),
        KeyBinding::new("shift-/", ShowShortcuts, Some(TABLE_KEYS)),
        KeyBinding::new("escape", ClearFilter, Some(FILTER_CONTEXT)),
        KeyBinding::new("escape", ClearLogFilter, Some(LOG_FILTER_CONTEXT)),
        KeyBinding::new(OPEN_PALETTE_KEY, OpenPalette, Some(KEY_CONTEXT)),
        KeyBinding::new(TOGGLE_SIDEBAR_KEY, ToggleSidebar, Some(KEY_CONTEXT)),
        KeyBinding::new(CLOSE_WINDOW_KEY, CloseWindow, Some(KEY_CONTEXT)),
    ]);
    cx.bind_keys(
        View::ALL.map(|view| KeyBinding::new(view.key(), ShowView { view }, Some(TABLE_KEYS))),
    );
    // GPUI Kit's table binds these keys to its own selection, which here only mirrors the
    // rows chosen whole, and binds `Tab` to move between cells, which would keep focus in
    // the table. Its bindings come first, so these replace them while the table has focus.
    cx.bind_keys([
        KeyBinding::new("down", NextRow, Some(TABLE_CONTEXT)),
        KeyBinding::new("up", PreviousRow, Some(TABLE_CONTEXT)),
        KeyBinding::new("home", FirstRow, Some(TABLE_CONTEXT)),
        KeyBinding::new("end", LastRow, Some(TABLE_CONTEXT)),
        KeyBinding::new("pagedown", NextPage, Some(TABLE_CONTEXT)),
        KeyBinding::new("pageup", PreviousPage, Some(TABLE_CONTEXT)),
        KeyBinding::new("escape", CloseDetails, Some(TABLE_CONTEXT)),
        KeyBinding::new("tab", FocusNextControl, Some(TABLE_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPreviousControl, Some(TABLE_CONTEXT)),
        KeyBinding::new("left", NoAction, Some(TABLE_CONTEXT)),
        KeyBinding::new("right", NoAction, Some(TABLE_CONTEXT)),
    ]);
}

const TITLE_BAR_HEIGHT: f32 = 40.;
const PANEL_HEADER_HEIGHT: f32 = 44.;
const CONTEXT_PICKER_CHAR_WIDTH: f32 = 7.5;
const CONTEXT_PICKER_CHROME: f32 = 40.;
const CONTEXT_PICKER_WIDTH: Range<f32> = 96.0..260.0;
const CONTEXT_BADGE_SIZE: f32 = 20.;
const FILTER_WIDTH: f32 = 240.;
const FILTER_MIN_WIDTH: f32 = 96.;
const COMPACT_HEADER_BELOW: f32 = 920.;
const NAMESPACE_PICKER_WIDTH: f32 = 148.;
const NAMESPACE_MENU_WIDTH: f32 = 240.;
const NOTICE_WIDTH: f32 = 420.;
const CONNECTION_DOT: f32 = 7.;
/// How many places **Back** remembers.
const MAX_HISTORY: usize = 50;

/// Window chrome with the traffic lights centred in the title bar.
pub fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(point(px(14.), px(13.))),
    }
}

/// A sheet shown over the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sheet {
    Shortcuts,
    About,
}

enum Session {
    NoClusters {
        error: Option<SharedString>,
    },
    ChooseContext,
    Connecting {
        context: String,
    },
    Failed {
        context: String,
        error: SharedString,
    },
    Connected {
        summary: String,
        version: String,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum NoticeAction {
    ReloadContexts,
    RetryConnection,
    ClearFilter,
    ShowAllNamespaces,
    ShowContextNamespace,
}

/// Whether the window may still choose the context's namespace for the user, because the
/// API refused to list pods across all namespaces, or has chosen it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ContextNamespace {
    #[default]
    Off,
    /// Waiting for the first pod list of this connection.
    Armed(String),
    /// Chosen, until the user chooses a namespace or another context.
    Chosen(String),
}

/// Where the user was before following a link, for **Back** to return to.
#[derive(Clone)]
struct Place {
    view: View,
    selected: Option<ObjectKey>,
    namespace: Option<String>,
    filter: String,
    selector: Option<SelectorFilter>,
    /// What **Back** names, such as `pod api-1`.
    label: String,
}

/// An object to select once its view has listed it.
#[derive(Clone)]
struct Pending {
    view: View,
    key: ObjectKey,
    /// Whether to clear the filter if it hides the object, as following a link does.
    reveal: bool,
}

pub(crate) struct Notice {
    title: SharedString,
    detail: Option<SharedString>,
    action: Option<(NoticeAction, SharedString)>,
}

impl Notice {
    fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            detail: None,
            action: None,
        }
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn action(mut self, action: NoticeAction, label: impl Into<SharedString>) -> Self {
        self.action = Some((action, label.into()));
        self
    }
}

pub struct Root {
    backend: Arc<Backend>,
    contexts: Option<Contexts>,
    picker: Option<Entity<ContextPicker>>,
    /// The pod view, which keeps watching while other views show, for the palette, the
    /// sidebar count and log tails that follow pods.
    pods: ResourcePanel<PodRow>,
    /// Every other view, created as it is first opened.
    panels: Vec<Box<dyn Panel>>,
    view: View,
    /// The view shown before this one, which choosing a namespace returns to.
    previous_view: View,
    /// Whether the cluster lacks `events.k8s.io/v1`, so Events come from the core API.
    core_events: bool,
    /// Hidden columns of views not yet opened, from saved state.
    saved_columns: std::collections::BTreeMap<String, Vec<String>>,
    filter: Entity<InputState>,
    namespace_picker: Entity<NamespacePicker>,
    namespace_options: Vec<String>,
    connection: Option<Connection>,
    logs: Option<Entity<LogPane>>,
    log_controls: LogControls,
    /// Says what the log dock is waiting for, or why a tail could not start.
    log_note: Option<SharedString>,
    workload_task: Option<Task<()>>,
    /// The places links were followed from, newest last.
    history: Vec<Place>,
    pending: Option<Pending>,
    palette: Option<Entity<CommandState>>,
    sheet: Option<Sheet>,
    /// Where focus was before the palette or a sheet opened, for closing it to return to.
    return_focus: Option<FocusHandle>,
    focus_handle: FocusHandle,
    session: Session,
    sidebar_collapsed: bool,
    saved_context: Option<String>,
    context_namespace: ContextNamespace,
    connect_task: Option<Task<()>>,
    _picker_events: Option<Subscription>,
    _filter_events: Subscription,
    _log_filter_events: [Subscription; 3],
    _namespace_events: Subscription,
    _appearance: Subscription,
    _bounds: Subscription,
    _focus_lost: Subscription,
}

impl Root {
    pub fn new(
        backend: Arc<Backend>,
        contexts: Result<Contexts, ClusterError>,
        saved: SavedState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pods = ResourcePanel::new(View::Pods, window, cx);
        let view = saved
            .view
            .as_deref()
            .and_then(View::from_name)
            .unwrap_or(View::Pods);
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(filter_placeholder(view)));
        let filter_events = cx.subscribe_in(
            &filter,
            window,
            |this, filter, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let filter = filter.read(cx).value().to_string();
                    this.apply_filter(filter, cx);
                }
                InputEvent::PressEnter { .. } => {
                    this.focus_table_or_root(window, cx);
                    this.open_selected(window, cx);
                }
                _ => {}
            },
        );
        let (log_controls, log_filter_events) = log_controls(window, cx);
        let namespace_options = vec![ALL_NAMESPACES.to_owned()];
        let namespace_picker = cx.new(|cx| {
            let options = SearchableVec::new(namespace_options.clone());
            SelectState::new(options, Some(IndexPath::default()), window, cx).searchable(true)
        });
        let namespace_events = cx.subscribe_in(
            &namespace_picker,
            window,
            |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(choice)) = event else {
                    return;
                };
                let namespace = (choice != ALL_NAMESPACES).then(|| choice.clone());
                this.set_namespace(namespace, window, cx);
                this.remember_state(window, cx);
            },
        );
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        let bounds = cx.observe_window_bounds(window, |this, window, cx| {
            this.remember_state(window, cx);
        });
        // Focus is lost when what held it goes away, such as a link in a details panel that
        // closed, or a table whose view changed. Keys then keep working on the table.
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            this.focus_table_or_root(window, cx);
        });
        let SavedState {
            sidebar_collapsed,
            context: saved_context,
            namespace,
            hidden_columns,
            view_columns,
            ..
        } = saved;
        let mut root = Self {
            backend,
            contexts: None,
            picker: None,
            pods,
            panels: Vec::new(),
            view: View::Pods,
            previous_view: View::Pods,
            core_events: false,
            saved_columns: view_columns,
            filter,
            namespace_picker,
            namespace_options,
            connection: None,
            logs: None,
            log_controls,
            log_note: None,
            workload_task: None,
            history: Vec::new(),
            pending: None,
            palette: None,
            sheet: None,
            return_focus: None,
            focus_handle,
            session: Session::ChooseContext,
            sidebar_collapsed,
            saved_context,
            context_namespace: ContextNamespace::Off,
            connect_task: None,
            _picker_events: None,
            _filter_events: filter_events,
            _log_filter_events: log_filter_events,
            _namespace_events: namespace_events,
            _appearance: appearance,
            _bounds: bounds,
            _focus_lost: focus_lost,
        };
        root.pods.set_hidden_columns(&hidden_columns, cx);
        root.set_namespace(namespace, window, cx);
        root.show_view(view, window, cx);
        root.load_contexts(contexts, window, cx);
        root
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    /// Focuses the table if it shows, so a screen reader reads its selected row, and
    /// otherwise the window, where the keys still work.
    fn focus_table_or_root(&self, window: &mut Window, cx: &mut App) {
        if self.notice(cx).is_none() {
            self.panel().table_focus(cx).focus(window, cx);
        } else {
            self.focus(window, cx);
        }
    }

    fn panel_for(&self, view: View) -> &dyn Panel {
        match view {
            View::Pods => &self.pods,
            view => self
                .panels
                .iter()
                .find(|panel| panel.view() == view)
                .map_or(&self.pods, Box::as_ref),
        }
    }

    fn panel_mut_for(&mut self, view: View) -> &mut dyn Panel {
        match view {
            View::Pods => &mut self.pods,
            view => match self.panels.iter().position(|panel| panel.view() == view) {
                Some(ix) => self.panels[ix].as_mut(),
                None => &mut self.pods,
            },
        }
    }

    /// Shows a pod snapshot as the pod watch would.
    #[cfg(test)]
    fn show_pods(
        &mut self,
        snapshot: std::sync::Arc<PodSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pods.table.update(cx, |table, cx| {
            crate::table::show_in_place(table, snapshot, cx);
        });
        self.after_snapshot(View::Pods, window, cx);
    }

    fn panel(&self) -> &dyn Panel {
        self.panel_for(self.view)
    }

    fn pod_table<'a>(&self, cx: &'a App) -> &'a ResourceTable<PodRow> {
        self.pods.table.read(cx).delegate()
    }

    /// Opens `view`, starting its watch, and closes the details of the view it leaves.
    /// Views other than Pods stop watching when left.
    pub(crate) fn show_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if view != View::Pods && !self.panels.iter().any(|panel| panel.view() == view) {
            let panel = new_panel(view, window, cx);
            if let Some(hidden) = self.saved_columns.get(view.name()) {
                panel.set_hidden_columns(hidden, cx);
            }
            panel.set_namespace(self.namespace(cx), cx);
            self.panels.push(panel);
        }
        let previous = self.view;
        if previous != view {
            self.previous_view = previous;
            self.panel_mut_for(previous).close_details(cx);
            if previous != View::Pods {
                self.panel_mut_for(previous).stop(cx);
            }
        }
        self.view = view;
        let filter = self.filter.read(cx).value().to_string();
        self.panel_for(view).set_filter(filter, cx);
        self.filter.update(cx, |input, cx| {
            input.set_placeholder(filter_placeholder(view), window, cx);
        });
        self.start_view(view, window, cx);
        self.update_pod_counts(cx);
        self.update_namespace_options(window, cx);
        cx.notify();
    }

    /// Counts pods for the current view, if it has a column that shows them.
    fn update_pod_counts(&self, cx: &mut App) {
        let panel = self.panel();
        if panel.counts_pods() {
            let counts = PodCounts::new(&self.pod_table(cx).snapshot().rows);
            panel.set_pod_counts(Arc::new(counts), cx);
        }
    }

    fn start_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let namespace = self.namespace(cx);
        let backend = self.backend.clone();
        self.panel_mut_for(view)
            .start(&backend, &connection, namespace, window, cx);
    }

    /// Runs after a view's table takes a new snapshot.
    pub(crate) fn after_snapshot(
        &mut self,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if view == View::Pods {
            self.choose_context_namespace(window, cx);
        }
        if view == View::Pods || view == self.view {
            self.update_namespace_options(window, cx);
        }
        if view == View::Pods && self.view != View::Pods {
            self.update_pod_counts(cx);
        }
        let not_served = matches!(
            self.panel_for(view).status(cx).error,
            Some(WatchError::NotServed { .. })
        );
        if view == View::Events && not_served && !self.core_events {
            self.core_events = true;
            cx.defer_in(window, |root, window, cx| root.use_core_events(window, cx));
        }
        self.panel_for(view).update_details(cx);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.view == view)
        {
            self.select_pending(window, cx);
        }
        cx.notify();
    }

    /// Replaces the Events panel with one over the core `v1` API.
    fn use_core_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.panels.iter().position(|p| p.view() == View::Events) else {
            return;
        };
        let panel = new_core_events_panel(window, cx);
        panel.set_hidden_columns(&owned(self.panels[ix].hidden_columns(cx)), cx);
        panel.set_namespace(self.namespace(cx), cx);
        panel.set_filter(self.filter.read(cx).value().to_string(), cx);
        self.panels[ix] = panel;
        if self.view == View::Events {
            self.start_view(View::Events, window, cx);
        }
        cx.notify();
    }

    fn load_contexts(
        &mut self,
        contexts: Result<Contexts, ClusterError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match contexts {
            Ok(contexts) if contexts.names().is_empty() => {
                self.session = Session::NoClusters { error: None };
            }
            Ok(contexts) => self.show_contexts(contexts, window, cx),
            Err(error) => {
                self.session = Session::NoClusters {
                    error: Some(error.to_string().into()),
                };
            }
        }
        cx.notify();
    }

    fn show_contexts(&mut self, contexts: Contexts, window: &mut Window, cx: &mut Context<Self>) {
        let names = contexts.names();
        let current = contexts
            .preferred(self.saved_context.as_deref())
            .map(str::to_owned);
        let selected = current
            .as_ref()
            .and_then(|current| names.iter().position(|name| name == current))
            .map(|row| IndexPath::default().row(row));
        let picker = cx.new(|cx| SelectState::new(names, selected, window, cx));
        self._picker_events = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(context)) = event else {
                    return;
                };
                this.connect(context.clone(), window, cx);
                this.remember_state(window, cx);
            },
        ));
        self.picker = Some(picker);
        self.contexts = Some(contexts);
        self.session = Session::ChooseContext;
        if let Some(current) = current {
            self.connect(current, window, cx);
        }
    }

    /// What to restore this window to. The saved context survives a window that has none
    /// selected, such as when the kubeconfig fails to load.
    pub(crate) fn saved_state(&self, window: &Window, cx: &App) -> SavedState {
        let context = self
            .picker
            .as_ref()
            .and_then(|picker| picker.read(cx).selected_value())
            .cloned()
            .or_else(|| self.saved_context.clone());
        let state = SavedState {
            context,
            sidebar_collapsed: self.sidebar_collapsed,
            namespace: self.chosen_namespace(cx),
            hidden_columns: owned(self.pods.hidden_columns(cx)),
            view: Some(self.view.name().to_owned()),
            view_columns: self.view_columns(cx),
            ..SavedState::default()
        };
        windows::with_placement(state, window, cx)
    }

    /// The namespace the user chose. One the window chose for them is not saved, so a context
    /// that may list across namespaces still opens on all of them.
    fn chosen_namespace(&self, cx: &App) -> Option<String> {
        let namespace = self.namespace(cx);
        match &self.context_namespace {
            ContextNamespace::Chosen(chosen) if namespace.as_ref() == Some(chosen) => None,
            _ => namespace,
        }
    }

    fn remember_state(&self, window: &Window, cx: &mut App) {
        windows::remember(self.saved_state(window, cx), cx);
    }

    fn connect(&mut self, context: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(contexts) = self.contexts.clone() else {
            return;
        };
        self.session = Session::Connecting {
            context: context.clone(),
        };
        self.connection = None;
        self.context_namespace = ContextNamespace::Off;
        self.logs = None;
        self.log_note = None;
        self.workload_task = None;
        self.history.clear();
        self.pending = None;
        self.close_details(cx);
        self.pods.stop(cx);
        for panel in &mut self.panels {
            panel.stop(cx);
        }
        self.update_namespace_options(window, cx);
        let connecting = self.backend.connect(contexts, context.clone());
        // Replacing the task drops any earlier session, so a slow context cannot
        // overwrite a newer choice.
        self.connect_task = Some(cx.spawn_in(window, async move |this, cx| {
            let connection = match connecting.await {
                Ok(connection) => connection,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.session = Session::Failed {
                            context,
                            error: error.to_string().into(),
                        };
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| {
                this.session = Session::Connected {
                    summary: connection_summary(&connection),
                    version: connection.server_version.clone(),
                };
                if connection.names_namespace && this.namespace(cx).is_none() {
                    this.context_namespace = ContextNamespace::Armed(connection.namespace.clone());
                }
                this.connection = Some(connection);
                this.start_view(View::Pods, window, cx);
                this.start_view(this.view, window, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn tail_logs(&mut self, cx: &mut Context<Self>) {
        let targets = self.log_targets(cx);
        self.start_tail(targets, cx);
    }

    fn start_tail(&mut self, targets: Vec<LogTarget>, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let tail = self.backend.tail_logs(connection, targets);
        self.show_tail(tail, cx);
    }

    /// Tails the pods that `follow` matches, as they come and go.
    fn start_follow(&mut self, follow: Follow, cx: &mut Context<Self>) {
        let (Some(connection), Some(feed)) = (&self.connection, self.pods.feed()) else {
            return;
        };
        let tail = self.backend.follow_logs(connection, feed, follow);
        self.show_tail(tail, cx);
    }

    fn show_tail(&mut self, tail: io::Result<LogTail>, cx: &mut Context<Self>) {
        self.workload_task = None;
        self.log_note = None;
        match tail {
            Ok(tail) => {
                let controls = self.log_controls.clone();
                let filter = self.log_filter(cx);
                let context = self.log_context(cx);
                self.logs = Some(cx.new(|cx| {
                    let mut logs = LogPane::new(tail, controls, filter, cx);
                    logs.set_context(context, cx);
                    logs
                }));
            }
            Err(error) => self.set_log_note(Some(format!("Failed to start log tail: {error}")), cx),
        }
        cx.notify();
    }

    fn set_log_note(&mut self, note: Option<String>, cx: &mut Context<Self>) {
        self.log_note = note.map(SharedString::from);
        if let Some(logs) = &self.logs {
            let note = self.log_note.clone();
            logs.update(cx, |logs, cx| logs.set_note(note, cx));
        }
        cx.notify();
    }

    /// Follows the selected workload's pods, or the workload that controls the selected pod,
    /// such as its Deployment.
    pub(crate) fn tail_workload(&mut self, cx: &mut Context<Self>) {
        if self.view != View::Pods {
            match self.panel().selected_follow(cx) {
                Some(follow) => self.start_follow(follow, cx),
                None => {
                    self.set_log_note(Some("Select a workload to tail its pods".to_owned()), cx)
                }
            }
            return;
        }
        let selected = self.pod_table(cx).selected_object();
        let (Some(row), Some(connection)) = (selected.cloned(), self.connection.as_ref()) else {
            self.set_log_note(Some("Select a pod to tail its workload".to_owned()), cx);
            return;
        };
        let name = row.name.clone();
        let finding = self.backend.workload_of(connection, row);
        self.set_log_note(Some(format!("Finding the workload of {name}")), cx);
        self.workload_task = Some(cx.spawn(async move |this, cx| {
            let found = finding.await;
            this.update(cx, |this, cx| match found {
                Ok(follow) => this.start_follow(follow, cx),
                Err(error) => {
                    this.workload_task = None;
                    this.set_log_note(Some(error.to_string()), cx);
                }
            })
            .ok();
        }));
    }

    /// Follows the pods that `selector` matches in the chosen namespace.
    fn tail_selector(&mut self, selector: &str, cx: &mut Context<Self>) {
        match Selector::parse(selector) {
            Ok(selector) => {
                let follow = Follow::selector(self.namespace(cx), selector);
                self.start_follow(follow, cx);
            }
            Err(error) => self.set_log_note(Some(error), cx),
        }
    }

    #[cfg(feature = "bench")]
    pub(crate) fn follow_selector(&mut self, selector: &str, cx: &mut Context<Self>) {
        self.tail_selector(selector, cx);
    }

    #[cfg(feature = "bench")]
    pub(crate) fn log_stats(
        &self,
        cx: &App,
    ) -> Option<(
        usize,
        Option<taffrail_core::Following>,
        u64,
        taffrail_core::LogReader,
    )> {
        Some(self.logs.as_ref()?.read(cx).stats())
    }

    /// The rows the current view's watch has listed, once it has.
    #[cfg(feature = "bench")]
    pub(crate) fn synced_row_count(&self, cx: &App) -> Option<usize> {
        let status = self.panel().status(cx);
        (self.connection.is_some() && status.synced).then_some(status.total)
    }

    #[cfg(feature = "bench")]
    pub(crate) fn scroll_rows_to(&mut self, row: usize, cx: &mut Context<Self>) {
        self.panel().scroll_to_row(row, cx);
    }

    /// Types `query` in the log filter and picks the severity choice at `severity`.
    #[cfg(feature = "preview")]
    pub(crate) fn set_log_filter(
        &mut self,
        query: &str,
        severity: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.log_controls
            .query
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.log_controls.severity.update(cx, |picker, cx| {
            picker.set_selected_index(Some(IndexPath::new(severity)), window, cx);
        });
        self.apply_log_filter(cx);
    }

    /// Picks the context choice at `index`.
    #[cfg(feature = "preview")]
    pub(crate) fn set_log_context(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.log_controls.context.update(cx, |picker, cx| {
            picker.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
        self.apply_log_filter(cx);
    }

    #[cfg(feature = "preview")]
    pub(crate) fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    pub(crate) fn open_details(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let backend = self.backend.clone();
        self.panel_mut_for(self.view)
            .open_details(backend, connection, cx);
        cx.notify();
    }

    pub(crate) fn close_details(&mut self, cx: &mut Context<Self>) {
        self.panel_mut_for(self.view).close_details(cx);
        cx.notify();
    }

    /// Follows the selected row where it leads, or opens its details, or selects the first
    /// row if nothing is selected.
    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel().has_selection(cx) {
            self.move_selection(1, window, cx);
            return;
        }
        match self.panel().selected_jump(cx) {
            Some(jump) => self.jump(jump, window, cx),
            None => self.open_details(cx),
        }
    }

    /// Goes where a row leads. Choosing a namespace returns to the view before, or to Pods
    /// if that view is not in namespaces.
    fn jump(&mut self, jump: Jump, window: &mut Window, cx: &mut Context<Self>) {
        match jump {
            Jump::Namespace(namespace) => {
                let back = self.previous_view;
                let returns = back != self.view && self.panel_for(back).namespaced();
                self.set_namespace(Some(namespace), window, cx);
                self.show_view(if returns { back } else { View::Pods }, window, cx);
            }
            Jump::Link(link) => self.open_link(link, window, cx),
        }
        self.remember_state(window, cx);
    }

    fn open_pod(&mut self, key: PodKey, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.history.clear();
        self.pending = None;
        self.show_view(View::Pods, window, cx);
        if !self.pods.select(key.clone(), cx) {
            self.clear_filter(window, cx);
            self.set_namespace(None, window, cx);
            self.pods.select(key, cx);
        }
        self.remember_state(window, cx);
        self.open_details(cx);
    }

    /// Shows the pods of the selected object: a workload's by its selector and in its
    /// namespace, or a node's in every namespace.
    fn show_workload_pods(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pods, namespace)) = self.panel().selected_pods(cx) else {
            return;
        };
        self.push_history(cx);
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.show_view(View::Pods, window, cx);
        self.pods.set_selector(Some(pods), cx);
        self.set_namespace(namespace, window, cx);
        self.remember_state(window, cx);
        cx.notify();
    }

    /// Where the user is now, for **Back** to return to.
    fn here(&self, cx: &App) -> Place {
        let selected = self.panel().selected_key(cx);
        let label = match &selected {
            Some((_, name)) => format!("{} {name}", self.view.kind().to_lowercase()),
            None => self.view.name().to_owned(),
        };
        Place {
            view: self.view,
            selected,
            namespace: self.namespace(cx),
            filter: self.filter.read(cx).value().to_string(),
            selector: self.panel().selector(cx),
            label,
        }
    }

    fn push_history(&mut self, cx: &App) {
        let place = self.here(cx);
        self.history.push(place);
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    /// Opens the view for a linked object's kind and selects the object. A namespaced
    /// object in another namespace than the chosen one switches to its namespace, and a
    /// filter that hides it clears. A kind without a view does nothing.
    fn open_link(&mut self, link: ObjectLink, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = View::for_kind(&link.kind) else {
            return;
        };
        self.close_overlays(window, cx);
        self.push_history(cx);
        let chosen = self.namespace(cx);
        if !link.namespace.is_empty() && chosen.is_some_and(|chosen| chosen != link.namespace) {
            self.set_namespace(Some(link.namespace.clone()), window, cx);
        }
        self.show_view(view, window, cx);
        self.pending = Some(Pending {
            view,
            key: link.key(),
            reveal: true,
        });
        self.select_pending(window, cx);
        self.remember_state(window, cx);
    }

    /// Returns to where the last link was followed from, with its view, namespace, filter
    /// and selection.
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.history.pop() else {
            return;
        };
        self.close_overlays(window, cx);
        self.pending = None;
        self.filter
            .update(cx, |filter, cx| filter.set_value(place.filter, window, cx));
        self.set_namespace(place.namespace, window, cx);
        self.show_view(place.view, window, cx);
        self.panel().set_selector(place.selector, cx);
        match place.selected {
            Some(key) => {
                self.pending = Some(Pending {
                    view: place.view,
                    key,
                    reveal: false,
                });
                self.select_pending(window, cx);
            }
            None => self.close_details(cx),
        }
        self.remember_state(window, cx);
        cx.notify();
    }

    /// Moves to the context's namespace when the first pod list of a connection is refused
    /// across all namespaces, as it is for a role bound in one namespace.
    fn choose_context_namespace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ContextNamespace::Armed(namespace) = &self.context_namespace else {
            return;
        };
        let namespace = namespace.clone();
        let snapshot = self.pod_table(cx).snapshot();
        match &snapshot.error {
            Some(WatchError::Forbidden {
                namespace: None, ..
            }) => {
                self.set_namespace(Some(namespace.clone()), window, cx);
                self.context_namespace = ContextNamespace::Chosen(namespace);
            }
            None if snapshot.synced => self.context_namespace = ContextNamespace::Off,
            _ => {}
        }
    }

    /// Selects the object a link or **Back** asked for, once its view has listed it. If the
    /// API refuses the list across all namespaces, the view moves to the object's namespace
    /// first. If it refuses that too, the view shows its **No permission** state.
    fn select_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if pending.view != self.view {
            self.pending = None;
            return;
        }
        let status = self.panel().status(cx);
        let namespace = &pending.key.0;
        match &status.error {
            Some(WatchError::Forbidden {
                namespace: None, ..
            }) if !namespace.is_empty() => {
                if self.namespace(cx).as_ref() != Some(namespace) {
                    self.set_namespace(Some(namespace.clone()), window, cx);
                    self.remember_state(window, cx);
                }
                return;
            }
            Some(_) if !status.synced => {
                self.pending = None;
                return;
            }
            _ if !status.synced => return,
            _ => {}
        }
        self.pending = None;
        if !self.panel().contains(&pending.key, cx) {
            return;
        }
        if !self.panel().select(pending.key.clone(), cx) && pending.reveal {
            self.clear_filter(window, cx);
            self.panel().select(pending.key, cx);
        }
        self.open_details(cx);
    }

    fn selected_context(&self, cx: &App) -> Option<String> {
        self.picker
            .as_ref()
            .and_then(|picker| picker.read(cx).selected_value())
            .cloned()
    }

    fn switch_context(&mut self, context: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = &self.picker {
            picker.update(cx, |picker, cx| {
                picker.set_selected_value(&context, window, cx);
            });
        }
        self.connect(context, window, cx);
        self.remember_state(window, cx);
    }

    /// The log filter the field and the severity picker spell.
    fn log_filter(&self, cx: &App) -> LogFilter {
        LogFilter {
            query: self.log_controls.query.read(cx).value().to_string(),
            severity: chosen_severity(self.log_controls.severity.read(cx).selected_value()),
        }
    }

    /// How many lines of a match's source the context picker asks for.
    fn log_context(&self, cx: &App) -> usize {
        chosen_context(self.log_controls.context.read(cx).selected_value())
    }

    fn apply_log_filter(&mut self, cx: &mut Context<Self>) {
        let filter = self.log_filter(cx);
        let context = self.log_context(cx);
        if let Some(logs) = &self.logs {
            logs.update(cx, |logs, cx| {
                logs.set_filter(filter, cx);
                logs.set_context(context, cx);
            });
        }
    }

    /// Clears the log filter's text, and its severity too if `severity`.
    fn clear_log_filter(&mut self, severity: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.log_controls
            .query
            .update(cx, |query, cx| query.set_value("", window, cx));
        if severity {
            self.log_controls.severity.update(cx, |picker, cx| {
                picker.set_selected_index(Some(IndexPath::default()), window, cx);
            });
        }
        self.apply_log_filter(cx);
    }

    /// Clears the filter text, and any selector narrowing the view.
    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.panel().set_selector(None, cx);
        self.apply_filter(String::new(), cx);
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.return_focus = window.focused(cx);
        let palette = cx.new(|cx| CommandState::new(window, cx));
        palette.update(cx, |palette, cx| palette.focus(window, cx));
        self.palette = Some(palette);
        cx.notify();
    }

    /// Opens a sheet over the panel. Focus moves to the window, so `Esc` closes it.
    fn open_sheet(&mut self, sheet: Sheet, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.return_focus = window.focused(cx);
        self.sheet = Some(sheet);
        self.focus(window, cx);
        cx.notify();
    }

    /// Runs after a palette command, which may itself have opened a sheet or moved focus.
    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.return_focus(window, cx);
            cx.notify();
        }
    }

    /// Closes an open palette or sheet, and returns focus to where it was before. With
    /// neither open, focus stays where it is, such as on the log timeline.
    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let palette = self.palette.take();
        let sheet = self.sheet.take();
        if palette.is_some() || sheet.is_some() {
            self.return_focus(window, cx);
            cx.notify();
        }
    }

    fn return_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.return_focus.take() {
            Some(focus) => focus.focus(window, cx),
            None => self.focus_table_or_root(window, cx),
        }
    }

    fn show_tab(&mut self, tab: PaneTab, cx: &mut Context<Self>) {
        self.panel().show_tab(tab, cx);
    }

    fn tail_selection(&mut self, cx: &mut Context<Self>) {
        if self.view != View::Pods {
            self.tail_workload(cx);
            return;
        }
        let selected = self.pod_table(cx).selection_targets();
        match selected {
            Some(targets) => self.start_tail(targets, cx),
            None => self.tail_logs(cx),
        }
    }

    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.remember_state(window, cx);
        cx.notify();
    }

    /// Moves the selection and opens its details. From the window, focus moves to the table,
    /// so a screen reader reads the row selected.
    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel().move_selection(delta, cx) {
            self.open_details(cx);
            if self.focus_handle.is_focused(window) {
                self.panel().table_focus(cx).focus(window, cx);
            }
        }
    }

    /// The rows `Page Up` and `Page Down` move by: one fewer than the table shows, so the
    /// row selected stays in view.
    fn page(&self, cx: &App) -> isize {
        self.panel().page_rows(cx).saturating_sub(1).max(1) as isize
    }

    /// Selects a row as a click would.
    #[cfg(feature = "preview")]
    pub(crate) fn select_first_row(&mut self, cx: &mut Context<Self>) {
        if self.panel().move_selection(1, cx) {
            self.open_details(cx);
        }
    }

    fn log_targets(&self, cx: &App) -> Vec<LogTarget> {
        self.pod_table(cx).log_targets()
    }

    /// The namespaces with pods, or with objects of the current view, after **All
    /// namespaces**. A chosen namespace stays listed after it disappears. Snapshots arrive
    /// up to 10 times a second, and most leave the namespaces alone.
    fn update_namespace_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut options: Vec<String> = owned(self.pod_table(cx).namespace_options());
        if self.view != View::Pods && self.panel().namespaced() {
            for namespace in self.panel().namespaces(cx) {
                if let Err(ix) = options[1..].binary_search(&namespace) {
                    options.insert(ix + 1, namespace);
                }
            }
        }
        if options == self.namespace_options {
            return;
        }
        self.namespace_options.clone_from(&options);
        self.namespace_picker.update(cx, |picker, cx| {
            picker.set_items(options.into(), window, cx);
        });
    }

    pub(crate) fn namespace(&self, cx: &App) -> Option<String> {
        self.pod_table(cx).namespace().map(str::to_owned)
    }

    pub(crate) fn set_namespace(
        &mut self,
        namespace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_namespace = ContextNamespace::Off;
        self.pods.set_namespace(namespace.clone(), cx);
        for panel in &self.panels {
            panel.set_namespace(namespace.clone(), cx);
        }
        self.update_namespace_options(window, cx);
        let choice = self
            .namespace(cx)
            .unwrap_or_else(|| ALL_NAMESPACES.to_owned());
        self.namespace_picker.update(cx, |picker, cx| {
            picker.set_selected_value(&choice, window, cx);
        });
        cx.notify();
    }

    fn apply_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        self.panel().set_filter(filter, cx);
        cx.notify();
    }

    /// The current view's hidden columns.
    pub(crate) fn hidden_columns(&self, cx: &App) -> Vec<&'static str> {
        self.panel().hidden_columns(cx)
    }

    /// Hides these columns of the current view and shows the rest.
    pub(crate) fn set_hidden_columns(
        &mut self,
        names: impl IntoIterator<Item = impl AsRef<str>>,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| name.as_ref().to_owned())
            .collect();
        self.panel().set_hidden_columns(&names, cx);
        cx.notify();
    }

    /// Hidden columns by view, for views other than Pods. Views not opened yet keep what
    /// was saved.
    fn view_columns(&self, cx: &App) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut columns = self.saved_columns.clone();
        for panel in &self.panels {
            let hidden = owned(panel.hidden_columns(cx));
            let name = panel.view().name().to_owned();
            if hidden.is_empty() {
                columns.remove(&name);
            } else {
                columns.insert(name, hidden);
            }
        }
        columns
    }

    fn toggle_column(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let mut hidden = self.hidden_columns(cx);
        match hidden.iter().position(|column| *column == name) {
            Some(ix) => {
                hidden.remove(ix);
            }
            None => hidden.push(name),
        }
        self.set_hidden_columns(hidden, cx);
    }

    fn run_notice_action(
        &mut self,
        action: NoticeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            NoticeAction::ReloadContexts => self.load_contexts(Contexts::load(), window, cx),
            NoticeAction::RetryConnection => {
                if let Session::Failed { context, .. } = &self.session {
                    self.connect(context.clone(), window, cx);
                }
            }
            NoticeAction::ClearFilter => self.clear_filter(window, cx),
            NoticeAction::ShowAllNamespaces => {
                self.set_namespace(None, window, cx);
                self.remember_state(window, cx);
            }
            NoticeAction::ShowContextNamespace => {
                let namespace = self.connection.as_ref().map(|c| c.namespace.clone());
                self.set_namespace(namespace, window, cx);
                self.remember_state(window, cx);
            }
        }
    }

    fn status(&self, cx: &App) -> String {
        match &self.session {
            Session::NoClusters { error: Some(error) } => error.to_string(),
            Session::NoClusters { error: None } => "No clusters found".into(),
            Session::ChooseContext => "Choose a context".into(),
            Session::Connecting { context } => format!("Connecting to {context}"),
            Session::Failed { error, .. } => error.to_string(),
            Session::Connected { summary, .. } => {
                pod_status(summary, self.pod_table(cx).snapshot())
            }
        }
    }

    fn notice(&self, cx: &App) -> Option<Notice> {
        let status = self.panel().status(cx);
        let name = self.view.name();
        let plural = name.to_lowercase();
        match &self.session {
            Session::NoClusters { error } => {
                let detail = error.clone().unwrap_or_else(|| {
                    "Taffrail reads KUBECONFIG from your shell, or ~/.kube/config. Add a cluster with your provider's CLI, then reload.".into()
                });
                Some(
                    Notice::new("No clusters found")
                        .detail(detail)
                        .action(NoticeAction::ReloadContexts, "Reload contexts"),
                )
            }
            Session::ChooseContext => Some(
                Notice::new("Choose a context")
                    .detail("Pick a cluster from the menu at the top of the window."),
            ),
            Session::Connecting { context } => {
                Some(Notice::new(format!("Connecting to {context}")))
            }
            Session::Failed { context, error } => Some(
                Notice::new(format!("Can't reach {context}"))
                    .detail(error.clone())
                    .action(NoticeAction::RetryConnection, "Try again"),
            ),
            Session::Connected { .. } => match status.error {
                Some(WatchError::Forbidden { message, .. }) if !self.panel().namespaced() => {
                    Some(cluster_forbidden_notice(name, message))
                }
                Some(WatchError::Forbidden { namespace, message }) => {
                    Some(self.forbidden_notice(name, namespace, message))
                }
                Some(WatchError::NotServed { message }) => {
                    Some(Notice::new(format!("This cluster doesn't serve {name}")).detail(message))
                }
                Some(WatchError::Failed { message }) if !status.synced => {
                    Some(Notice::new(format!("Can't list {plural}")).detail(message))
                }
                _ if !status.synced => Some(Notice::new(format!("Loading {plural}"))),
                _ if status.visible > 0 => None,
                _ => Some(self.panel().empty_notice(cx)),
            },
        }
    }

    /// The **No permission** state, which says where listing failed and what to try.
    fn forbidden_notice(&self, name: &str, namespace: Option<String>, message: String) -> Notice {
        let detail = if message.is_empty() {
            "The API refused the list.".to_owned()
        } else {
            message
        };
        match namespace {
            Some(namespace) => {
                Notice::new(format!("You can't list {name} in {namespace}")).detail(detail)
            }
            None => {
                let notice = Notice::new(format!("You can't list {name} across all namespaces"))
                    .detail(format!("{detail}. Choose a namespace you can read."));
                match &self.connection {
                    Some(connection) => notice.action(
                        NoticeAction::ShowContextNamespace,
                        format!("Show namespace {}", connection.namespace),
                    ),
                    None => notice,
                }
            }
        }
    }

    /// Says why the window shows the namespace it chose for the user.
    fn context_namespace_note(&self) -> Option<String> {
        match &self.context_namespace {
            ContextNamespace::Chosen(namespace) => Some(format!(
                "Showing {namespace}, the namespace your context names, because you can't list pods across all namespaces."
            )),
            _ => None,
        }
    }

    fn connection_state(&self, cx: &App) -> (Hsla, &'static str, Option<SharedString>) {
        let theme = cx.theme();
        match &self.session {
            Session::Connected { version, .. } => {
                let failing = matches!(
                    self.pod_table(cx).snapshot().error,
                    Some(WatchError::Failed { .. })
                );
                if failing {
                    (theme.warning, "Updates failing", None)
                } else {
                    (theme.success, "Connected", Some(version.clone().into()))
                }
            }
            Session::Connecting { .. } => (theme.warning, "Connecting", None),
            Session::Failed { .. } => (theme.danger, "Not connected", None),
            Session::NoClusters { .. } | Session::ChooseContext => {
                (theme.muted_foreground, "Not connected", None)
            }
        }
    }

    /// What the sidebar shows for each view: its count once listed, and whether the API
    /// refused it.
    fn sidebar_items(&self, cx: &App) -> Vec<SidebarItem> {
        View::ALL
            .into_iter()
            .map(|view| {
                let status = self.panel_for(view).status(cx);
                let listed = status.watching && status.synced && status.error.is_none();
                let present = view == View::Pods || self.panels.iter().any(|p| p.view() == view);
                SidebarItem {
                    view,
                    active: view == self.view,
                    count: (present && listed).then_some(status.total),
                    forbidden: present
                        && status.watching
                        && status.error.as_ref().is_some_and(WatchError::is_forbidden),
                }
            })
            .collect()
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let collapsed = self.sidebar_collapsed;
        let toggle_label = if collapsed {
            "Show sidebar"
        } else {
            "Hide sidebar"
        };
        let context = self
            .picker
            .as_ref()
            .and_then(|picker| picker.read(cx).selected_value())
            .cloned()
            .unwrap_or_default();
        let picker_width = context_picker_width(&context);
        let initial = context
            .chars()
            .next()
            .map(|initial| initial.to_ascii_uppercase().to_string());
        TitleBar::new()
            .h(px(TITLE_BAR_HEIGHT))
            .bg(cx.theme().title_bar)
            .border_b_0()
            .child(
                h_flex()
                    .h_full()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .small()
                            .icon(Icon::new(if collapsed {
                                IconName::PanelLeftOpen
                            } else {
                                IconName::PanelLeftClose
                            }))
                            .accessibility_label(toggle_label)
                            .tooltip(toggle_label)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(window, cx);
                            })),
                    )
                    .when_some(initial, |bar, initial| {
                        bar.child(
                            div()
                                .size(px(CONTEXT_BADGE_SIZE))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().ring)
                                .text_color(cx.theme().background)
                                .text_xs()
                                .font_semibold()
                                .child(initial),
                        )
                    })
                    .when_some(self.picker.as_ref(), |bar, picker| {
                        bar.child(
                            div().w(px(picker_width)).child(
                                Select::new(picker)
                                    .id("context-picker")
                                    .small()
                                    .appearance(false)
                                    .accessibility_label("Kubernetes context")
                                    .placeholder("Choose a context"),
                            ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .id("open-palette")
                    .test_support()
                    .role(Role::Button)
                    .aria_label("Search or run a command")
                    .mr_2()
                    .h(px(SEARCH_HEIGHT))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .hover(|pill| pill.bg(cx.theme().secondary_hover))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
                    .child(Icon::new(IconName::Search).size_3())
                    .child("Search or run a command")
                    .child(PALETTE_KEY),
            )
    }

    fn palette_source<'a>(&'a self, query: &'a str, cx: &'a App) -> PaletteSource<'a> {
        let table = self.pod_table(cx);
        PaletteSource {
            query,
            view: self.view,
            rows: &table.snapshot().rows,
            contexts: self
                .contexts
                .as_ref()
                .map(Contexts::names)
                .unwrap_or_default(),
            current_context: self.selected_context(cx),
            namespaces: table.namespaces(),
            namespace: table.namespace(),
            has_selection: self.panel().details().is_some(),
            has_follow: self.panel().selected_follow(cx).is_some(),
            has_pods: self.panel().selected_pods(cx).is_some(),
            jump: self
                .panel()
                .selected_jump(cx)
                .filter(|_| self.panel().details().is_some())
                .map(|jump| jump.label()),
            has_tail: self.logs.is_some(),
            connected: self.connection.is_some(),
            back: self.history.last().map(|place| place.label.clone()),
            selector: self.panel().selector(cx).map(|selector| selector.label),
            sort: match self.notice(cx) {
                None => self.panel().sort_commands(cx),
                Some(_) => Vec::new(),
            },
        }
    }

    fn render_palette(
        &self,
        palette: &Entity<CommandState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let query = palette.read(cx).query(cx);
        let entries = groups(self.palette_source(&query, cx));
        let root = cx.entity();
        let closing = root.clone();
        let mut command = Command::new(palette)
            .placeholder("Search pods or run a command")
            .max_h(px(PALETTE_HEIGHT))
            .empty(|_, _, _| div().p_4().text_sm().child("No matches"))
            .on_query(move |_, _, cx| root.update(cx, |_, cx| cx.notify()))
            .on_confirm(move |_, window, cx| {
                closing.update(cx, |root, cx| root.close_palette(window, cx));
            });
        for group in entries {
            command = command.group(group);
        }
        let palette = div()
            .id("palette")
            .test_support()
            .role(Role::Dialog)
            .aria_label("Command palette")
            .child(command);
        render_overlay(PALETTE_WIDTH, palette.into_any_element(), cx)
    }

    fn render_connection(&self, cx: &App) -> impl IntoElement + use<> {
        let (color, label, version) = self.connection_state(cx);
        let muted = cx.theme().muted_foreground;
        let collapsed = self.sidebar_collapsed;
        h_flex()
            .id("status")
            .role(Role::Status)
            .test_support()
            .aria_label(self.status(cx))
            .w_full()
            .h_7()
            .px_2()
            .gap_2()
            .items_center()
            .text_xs()
            .text_color(muted)
            .when(collapsed, |row| row.justify_center())
            .child(div().size(px(CONNECTION_DOT)).rounded_full().bg(color))
            .when(!collapsed, |row| {
                row.child(div().flex_1().min_w_0().truncate().child(label))
                    .when_some(version, |row, version| row.child(version))
            })
    }

    fn render_panel_header(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let compact = window.viewport_size().width < px(COMPACT_HEADER_BELOW);
        let hidden = self.hidden_columns(cx);
        let columns = self.panel().column_names();
        let view = self.view;
        let (label, can_tail): (SharedString, bool) = if view == View::Pods {
            let targets = self.log_targets(cx);
            let pods = distinct_pods(&targets);
            let label = tail_label(pods, self.pod_table(cx).visible_rows());
            (label.into(), pods > 0)
        } else {
            let workload = self.panel().selected_follow(cx).is_some();
            ("Tail its pods".into(), workload)
        };
        let selector = self.panel().selector(cx);
        let back = self
            .history
            .last()
            .map(|place| format!("Back to {}", place.label));
        let namespaced = self.panel().namespaced();
        let tails = view == View::Pods || view.group() == "Workloads";
        let muted = cx.theme().muted_foreground;
        h_flex()
            .h(px(PANEL_HEADER_HEIGHT))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_sm()
            .when_some(back, |header, back| {
                header.child(
                    Button::new("go-back")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::ArrowLeft))
                        .accessibility_label(back.clone())
                        .tooltip(back)
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(GoBack), cx)),
                )
            })
            .when(!compact, |header| {
                header
                    .child(div().text_color(muted).child(view.group()))
                    .child(div().text_color(muted).child("/"))
            })
            .child(div().font_medium().child(view.name()))
            .child(div().flex_1())
            .when_some(selector, |header, selector| {
                header.child(render_selector_chip(selector.label, cx))
            })
            .when(namespaced, |header| {
                header.child(
                    div().w(px(NAMESPACE_PICKER_WIDTH)).flex_shrink_0().child(
                        Select::new(&self.namespace_picker)
                            .id("namespace-picker")
                            .small()
                            .menu_width(px(NAMESPACE_MENU_WIDTH))
                            .accessibility_label("Namespace")
                            .search_placeholder("Search namespaces"),
                    ),
                )
            })
            .when(!namespaced, |header| {
                header.child(
                    h_flex()
                        .id("cluster-wide")
                        .test_support()
                        .role(Role::Label)
                        .aria_label(format!(
                            "{} are cluster-wide, so the namespace does not apply",
                            view.name()
                        ))
                        .w(px(NAMESPACE_PICKER_WIDTH))
                        .flex_shrink_0()
                        .gap_1p5()
                        .items_center()
                        .text_color(muted)
                        .child(Icon::new(IconName::Globe).small())
                        .child("Cluster-wide"),
                )
            })
            .child(
                div()
                    .key_context(FILTER_CONTEXT)
                    .w(px(FILTER_WIDTH))
                    .min_w(px(FILTER_MIN_WIDTH))
                    .child(
                        Input::new(&self.filter)
                            .id("pod-filter")
                            .small()
                            .prefix(Icon::new(IconName::Search).small().text_color(muted))
                            .aria_label(filter_placeholder(view)),
                    ),
            )
            .child(
                div().flex_shrink_0().child(
                    Button::new("columns")
                        .small()
                        .outline()
                        .icon(Icon::new(IconName::Columns3))
                        .accessibility_label("Columns")
                        .map(|button| {
                            if compact {
                                button.tooltip("Columns")
                            } else {
                                button.label("Columns")
                            }
                        })
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            columns_menu(menu, &columns, &hidden)
                        }),
                ),
            )
            .when(tails, |header| {
                header.child(
                    div().flex_shrink_0().child(
                        Button::new("tail-logs")
                            .small()
                            .outline()
                            .icon(Icon::new(IconName::ScrollText))
                            .label(label)
                            .disabled(!can_tail || self.connection.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if view == View::Pods {
                                    this.tail_logs(cx);
                                } else {
                                    this.tail_workload(cx);
                                }
                            })),
                    ),
                )
            })
    }

    fn render_notice(&self, notice: Notice, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex()
            .id("notice")
            .test_support()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(
                div()
                    .id("notice-title")
                    .test_support()
                    .role(Role::Heading)
                    .aria_label(notice.title.clone())
                    .font_medium()
                    .child(notice.title),
            )
            .when_some(notice.detail, |content, detail| {
                content.child(
                    div()
                        .max_w(px(NOTICE_WIDTH))
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
            })
            .when_some(notice.action, |content, (action, label)| {
                content.child(
                    div().mt_2().child(
                        Button::new("notice-action")
                            .small()
                            .outline()
                            .label(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_notice_action(action, window, cx);
                            })),
                    ),
                )
            })
    }
}

/// The **No permission** state for a kind that lives outside namespaces, where choosing a
/// namespace cannot help.
fn cluster_forbidden_notice(name: &str, message: String) -> Notice {
    let detail = if message.is_empty() {
        "The API refused the list.".to_owned()
    } else {
        message
    };
    Notice::new(format!("You can't list {name} in this cluster")).detail(format!(
        "{detail}. {name} are not in namespaces, so listing them needs a cluster-wide role."
    ))
}

fn columns_menu(menu: PopupMenu, columns: &[&'static str], hidden: &[&'static str]) -> PopupMenu {
    columns.iter().enumerate().fold(menu, |menu, (ix, &name)| {
        menu.menu_with_check_and_disabled(
            name,
            !hidden.contains(&name),
            Box::new(ToggleColumn { name }),
            ix == 0,
        )
    })
}

fn filter_placeholder(view: View) -> String {
    format!("Filter {}", view.name().to_lowercase())
}

fn owned(names: Vec<&str>) -> Vec<String> {
    names.into_iter().map(str::to_owned).collect()
}

/// Says why a table has no rows, and offers what would bring some back.
pub(crate) fn no_rows_notice<R: TableRow>(table: &ResourceTable<R>) -> Notice {
    let plural = R::PLURAL;
    if let Some(namespace) = table.empty_namespace() {
        return Notice::new(format!("No {plural} in {namespace}"))
            .action(NoticeAction::ShowAllNamespaces, "Show all namespaces");
    }
    let filter = table.filter();
    let selector = table.selector();
    if filter.is_empty() && selector.is_none() {
        return Notice::new(format!("No {plural} in this cluster"));
    }
    let what = match (filter, selector) {
        ("", Some(selector)) => selector.label.clone(),
        (filter, _) => format!("\"{filter}\""),
    };
    Notice::new(format!("No {plural} match {what}"))
        .action(NoticeAction::ClearFilter, "Clear filter")
}

/// A muted line above the table that says why the window chose its namespace.
fn render_context_namespace_note(note: String, cx: &App) -> impl IntoElement + use<> {
    div()
        .id("context-namespace-note")
        .test_support()
        .role(Role::Label)
        .aria_label(note.clone())
        .flex_shrink_0()
        .px_3()
        .py_1p5()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(note)
}

/// The pill that says a label selector narrows the table, with a button that clears it.
fn render_selector_chip(label: String, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .id("selector-filter")
        .test_support()
        .role(Role::Group)
        .aria_label(format!("Showing pods of {label}"))
        .flex_shrink_1()
        .min_w_0()
        .h(px(SEARCH_HEIGHT))
        .pl_2()
        .gap_1()
        .items_center()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .text_xs()
        .child(div().min_w_0().truncate().child(label))
        .child(
            Button::new("clear-selector")
                .ghost()
                .xsmall()
                .icon(Icon::new(IconName::X))
                .accessibility_label("Show all pods")
                .tooltip("Show all pods")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ClearSelector), cx)),
        )
}

fn connection_summary(connection: &Connection) -> String {
    format!(
        "Connected to {}, server {}",
        connection.context, connection.server_version
    )
}

fn pod_status(summary: &str, snapshot: &PodSnapshot) -> String {
    match (&snapshot.error, snapshot.synced) {
        (Some(error), _) => format!("{summary}. Pod watch failed: {error}"),
        (None, true) => format!("{summary}. Pods: {}", snapshot.rows.len()),
        (None, false) => format!("{summary}. Loading pods"),
    }
}

/// The log dock's filter field and severity picker, and the subscriptions that apply them.
fn log_controls(window: &mut Window, cx: &mut Context<Root>) -> (LogControls, [Subscription; 3]) {
    let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter lines"));
    let severity = cx.new(|cx| {
        let choices = SEVERITY_CHOICES.map(|(label, _)| label).to_vec();
        SeverityPicker::new(choices, Some(IndexPath::default()), window, cx)
    });
    let query_events = cx.subscribe_in(
        &query,
        window,
        |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.apply_log_filter(cx),
            InputEvent::PressEnter { .. } => this.focus_table_or_root(window, cx),
            _ => {}
        },
    );
    let severity_events = cx.subscribe_in(
        &severity,
        window,
        |this, _, _: &SelectEvent<Vec<&'static str>>, _, cx| this.apply_log_filter(cx),
    );
    let context = cx.new(|cx| {
        let choices = CONTEXT_CHOICES.map(|(label, _)| label).to_vec();
        LineContextPicker::new(choices, Some(IndexPath::default()), window, cx)
    });
    let context_events = cx.subscribe_in(
        &context,
        window,
        |this, _, _: &SelectEvent<Vec<&'static str>>, _, cx| this.apply_log_filter(cx),
    );
    (
        LogControls {
            query,
            severity,
            context,
        },
        [query_events, severity_events, context_events],
    )
}

/// Fits the picker to the context name, as the title bar has no use for a wide control.
fn context_picker_width(context: &str) -> f32 {
    let text = context.chars().count() as f32 * CONTEXT_PICKER_CHAR_WIDTH;
    (text + CONTEXT_PICKER_CHROME).clamp(CONTEXT_PICKER_WIDTH.start, CONTEXT_PICKER_WIDTH.end)
}

fn distinct_pods(targets: &[LogTarget]) -> usize {
    let mut pods: Vec<_> = targets
        .iter()
        .map(|target| (&target.namespace, &target.pod))
        .collect();
    pods.dedup();
    pods.len()
}

fn tail_label(pods: usize, matching: usize) -> String {
    match pods {
        0 => "Tail logs".to_owned(),
        1 => "Tail 1 pod".to_owned(),
        pods if matching > MAX_TAIL_PODS => format!("Tail first {pods} pods"),
        pods => format!("Tail {pods} pods"),
    }
}

/// The log dock's header alone, before any tail has started.
fn render_log_note(note: SharedString, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .h(px(BAR_HEIGHT))
        .flex_shrink_0()
        .px_3()
        .gap_2()
        .items_center()
        .border_t_1()
        .border_color(cx.theme().border)
        .text_sm()
        .child(div().font_medium().child("Logs"))
        .child(
            div()
                .id("log-summary")
                .test_support()
                .role(Role::Status)
                .aria_label(note.clone())
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .child(note),
        )
}

/// A centred sheet over the window. Clicking outside it closes it.
fn render_overlay(
    width: f32,
    content: AnyElement,
    cx: &mut Context<Root>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    div()
        .id("overlay")
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .justify_center()
        .items_start()
        .pt(px(PALETTE_TOP))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_overlays(window, cx)),
        )
        .child(
            div()
                .id("overlay-sheet")
                .w(px(width))
                .overflow_hidden()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar_items = self.sidebar_items(cx);
        let connection = self.render_connection(cx);
        let title_bar = self.render_title_bar(cx);
        let header = self.render_panel_header(window, cx);
        let content = match self.notice(cx) {
            Some(notice) => self.render_notice(notice, cx).into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .child(self.panel().table())
                .into_any_element(),
        };
        let content = v_flex()
            .size_full()
            .when_some(self.context_namespace_note(), |column, note| {
                column.child(render_context_namespace_note(note, cx))
            })
            .child(content);
        let details = self.panel().details();
        let dock = dock_height(window.viewport_size().height);
        let border = cx.theme().border;
        v_flex()
            .size_full()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NextRow, window, cx| {
                this.move_selection(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousRow, window, cx| {
                this.move_selection(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FirstRow, window, cx| {
                this.move_selection(-isize::MAX, window, cx);
            }))
            .on_action(cx.listener(|this, _: &LastRow, window, cx| {
                this.move_selection(isize::MAX, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextPage, window, cx| {
                let page = this.page(cx);
                this.move_selection(page, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousPage, window, cx| {
                let page = this.page(cx);
                this.move_selection(-page, window, cx);
            }))
            .on_action(cx.listener(|_, _: &FocusNextControl, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &FocusPreviousControl, window, cx| {
                window.focus_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &CloseDetails, window, cx| {
                if this.sheet.is_some() {
                    this.close_overlays(window, cx);
                } else {
                    this.close_details(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                this.close_overlays(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearFilter, window, cx| {
                if this.filter.read(cx).value().is_empty() {
                    this.focus_table_or_root(window, cx);
                } else {
                    this.clear_filter(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClearLogFilter, window, cx| {
                if this.log_controls.query.read(cx).value().is_empty() {
                    this.focus_table_or_root(window, cx);
                } else {
                    this.clear_log_filter(false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ResetLogFilter, window, cx| {
                this.clear_log_filter(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusLogFilter, window, cx| {
                this.close_overlays(window, cx);
                if this.logs.is_some() {
                    this.log_controls.query.focus_handle(cx).focus(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                // A focused button or link takes `Enter` itself.
                if this.focus_handle.is_focused(window) || this.panel().table_focused(window, cx) {
                    this.open_selected(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &FollowSelected, window, cx| {
                this.close_overlays(window, cx);
                if let Some(jump) = this.panel().selected_jump(cx) {
                    this.jump(jump, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowShortcuts, window, cx| {
                this.open_sheet(Sheet::Shortcuts, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowAbout, window, cx| {
                this.open_sheet(Sheet::About, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                this.close_overlays(window, cx);
                this.filter.focus_handle(cx).focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TailSelection, window, cx| {
                this.close_overlays(window, cx);
                this.tail_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &TailWorkload, window, cx| {
                this.close_overlays(window, cx);
                this.tail_workload(cx);
            }))
            .on_action(cx.listener(|this, action: &TailSelector, window, cx| {
                this.close_overlays(window, cx);
                this.tail_selector(&action.selector, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSummary, window, cx| {
                this.close_overlays(window, cx);
                this.show_tab(PaneTab::Summary, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowEvents, window, cx| {
                this.close_overlays(window, cx);
                this.show_tab(PaneTab::Events, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowYaml, window, cx| {
                this.close_overlays(window, cx);
                this.show_tab(PaneTab::Yaml, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleFollow, window, cx| {
                this.close_overlays(window, cx);
                if let Some(logs) = &this.logs {
                    logs.update(cx, |logs, cx| logs.toggle_following(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &NextError, window, cx| {
                this.close_overlays(window, cx);
                if let Some(logs) = &this.logs {
                    logs.update(cx, |logs, cx| logs.jump_to_error(true, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousError, window, cx| {
                this.close_overlays(window, cx);
                if let Some(logs) = &this.logs {
                    logs.update(cx, |logs, cx| logs.jump_to_error(false, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CopyLines, window, cx| {
                this.close_overlays(window, cx);
                if let Some(logs) = &this.logs {
                    logs.update(cx, |logs, cx| logs.copy_selection(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ExportLines, window, cx| {
                this.close_overlays(window, cx);
                if let Some(logs) = &this.logs {
                    logs.update(cx, |logs, cx| logs.export_lines(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                this.close_overlays(window, cx);
                this.toggle_sidebar(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadContexts, window, cx| {
                this.close_overlays(window, cx);
                this.load_contexts(Contexts::load(), window, cx);
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|this, action: &SwitchContext, window, cx| {
                this.close_overlays(window, cx);
                this.switch_context(action.name.to_string(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &SwitchNamespace, window, cx| {
                this.close_overlays(window, cx);
                let namespace = action.namespace.as_ref().map(ToString::to_string);
                this.set_namespace(namespace, window, cx);
                this.remember_state(window, cx);
            }))
            .on_action(cx.listener(|this, action: &OpenPod, window, cx| {
                this.open_pod(action.key.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &OpenLink, window, cx| {
                this.open_link(action.link.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                this.go_back(window, cx);
            }))
            .on_action(cx.listener(|this, action: &ShowView, window, cx| {
                this.close_overlays(window, cx);
                this.history.clear();
                this.pending = None;
                this.show_view(action.view, window, cx);
                this.remember_state(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowPods, window, cx| {
                this.close_overlays(window, cx);
                this.show_workload_pods(window, cx);
            }))
            .on_action(cx.listener(|this, action: &SortBy, window, cx| {
                this.close_overlays(window, cx);
                this.panel().cycle_sort(action.column, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ClearSelector, _, cx| {
                this.panel().set_selector(None, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &ToggleColumn, window, cx| {
                this.toggle_column(action.name, cx);
                this.remember_state(window, cx);
            }))
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .child(title_bar)
            .when_some(self.palette.clone(), |root, palette| {
                root.child(deferred(self.render_palette(&palette, cx)).priority(1))
            })
            .when_some(self.sheet, |root, sheet| {
                let (width, content) = match sheet {
                    Sheet::Shortcuts => (SHORTCUTS_WIDTH, render_shortcuts(cx).into_any_element()),
                    Sheet::About => (ABOUT_WIDTH, render_about(cx).into_any_element()),
                };
                root.child(deferred(render_overlay(width, content, cx)).priority(1))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(resource_sidebar(
                        &sidebar_items,
                        self.sidebar_collapsed,
                        connection,
                    ))
                    .child(
                        div().flex_1().min_w_0().h_full().pr_2().pb_2().child(
                            v_flex()
                                .size_full()
                                .overflow_hidden()
                                .rounded(cx.theme().radius_lg)
                                .border_1()
                                .border_color(border)
                                .bg(cx.theme().background)
                                .child(header)
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .min_h_0()
                                        .items_start()
                                        .child(v_flex().flex_1().min_w_0().h_full().child(content))
                                        .when_some(details, |row, details| {
                                            row.child(
                                                div()
                                                    .w(px(DETAILS_WIDTH))
                                                    .flex_shrink_0()
                                                    .h_full()
                                                    .border_l_1()
                                                    .border_color(border)
                                                    .child(details),
                                            )
                                        }),
                                )
                                .when_some(self.logs.clone(), |panel, logs| {
                                    panel.child(
                                        div()
                                            .h(dock)
                                            .flex_shrink_0()
                                            .border_t_1()
                                            .border_color(border)
                                            .child(logs),
                                    )
                                })
                                .when(self.logs.is_none(), |panel| {
                                    panel.when_some(self.log_note.clone(), |panel, note| {
                                        panel.child(render_log_note(note, cx))
                                    })
                                }),
                        ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        BACK_KEY, ContextNamespace, OPEN_PALETTE_KEY, Root, distinct_pods, no_rows_notice,
        pod_status, tail_label,
    };
    use crate::fake_api::{Fixture, kubeconfig, serve};
    use crate::logs::gap_text;
    use crate::pods::PodTable;
    use crate::views::View;

    use gpui_kit::SharedString;
    use gpui_kit::component::table::TableDelegate;
    use gpui_kit::test::{TestAppContextExt, TestQueryExt, TestWindowExt};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Bounds, Entity, Modifiers, Point, TestAppContext,
        WindowBounds, WindowOptions, px, size,
    };
    use taffrail_core::{
        Backend, Contexts, Gap, LogTarget, PodRow, PodSnapshot, SavedState, WatchError,
    };

    const POD_LIST_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const TWO_NAMESPACES_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}},{"metadata":{"namespace":"web","name":"front-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const POD_BODY: &str = r#"{"kind":"Pod","apiVersion":"v1","metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app","image":"api:1"}]},"status":{"phase":"Running"}}"#;
    const EVENTS_BODY: &str = r#"{"kind":"EventList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"e1","namespace":"shop"},"involvedObject":{"kind":"Pod","name":"api-1"},"type":"Warning","reason":"BackOff","message":"Back-off restarting failed container"}]}"#;
    const LOG_BODY: &str = "2026-01-01T00:00:00Z first line\n";
    const SEVERITY_LOG_BODY: &str = concat!(
        "2026-01-01T00:00:00Z ERROR payment failed\n",
        r#"2026-01-01T00:00:01Z {"level":"warn","ts":1.7e9,"msg":"slow query","ms":1204}"#,
        "\n",
        r#"2026-01-01T00:00:02Z {"level":"debug","seq":3}"#,
        "\n",
    );
    const OWNED_POD_LIST_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","uid":"u1","resourceVersion":"1","labels":{"app":"api"},"ownerReferences":[{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"api-7f9c","uid":"r","controller":true}]},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}},{"metadata":{"namespace":"shop","name":"web-1","uid":"u2","resourceVersion":"1","labels":{"app":"web"}},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const REPLICA_SET_BODY: &str = r#"{"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{"name":"api-7f9c","namespace":"shop","ownerReferences":[{"apiVersion":"apps/v1","kind":"Deployment","name":"api","uid":"d","controller":true}]},"spec":{"selector":{"matchLabels":{"app":"api","pod-template-hash":"7f9c"}}}}"#;
    const DEPLOYMENT_BODY: &str = r#"{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"api","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"api"}},"template":{}}}"#;
    const DEPLOYMENT_LIST_BODY: &str = r#"{"kind":"DeploymentList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api","resourceVersion":"1","generation":1},"spec":{"replicas":1,"selector":{"matchLabels":{"app":"api"}},"template":{}},"status":{"observedGeneration":1,"replicas":1,"readyReplicas":1,"updatedReplicas":1,"availableReplicas":1}}]}"#;
    const DEPLOYMENTS: &str = "/apis/apps/v1/deployments";
    const STATEFUL_SET_BODY: &str = r#"{"apiVersion":"apps/v1","kind":"StatefulSet","metadata":{"name":"api","namespace":"shop"},"spec":{"serviceName":"api","selector":{"matchLabels":{"app":"api"}},"template":{}}}"#;
    const STATEFUL_SET_LIST_BODY: &str = r#"{"kind":"StatefulSetList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api","resourceVersion":"1","generation":1},"spec":{"replicas":1,"serviceName":"api","selector":{"matchLabels":{"app":"api"}},"template":{}},"status":{"observedGeneration":1,"replicas":1,"readyReplicas":1,"currentReplicas":1,"updatedReplicas":1,"currentRevision":"api-1","updateRevision":"api-1"}}]}"#;
    const STATEFUL_SETS: &str = "/apis/apps/v1/statefulsets";
    const DAEMON_SET_BODY: &str = r#"{"apiVersion":"apps/v1","kind":"DaemonSet","metadata":{"name":"api","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"api"}},"template":{}}}"#;
    const DAEMON_SET_LIST_BODY: &str = r#"{"kind":"DaemonSetList","apiVersion":"apps/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api","resourceVersion":"1","generation":1},"spec":{"selector":{"matchLabels":{"app":"api"}},"template":{"spec":{"nodeSelector":{"kubernetes.io/os":"linux"},"containers":[{"name":"app"}]}}},"status":{"observedGeneration":1,"desiredNumberScheduled":2,"currentNumberScheduled":2,"numberReady":2,"updatedNumberScheduled":2,"numberAvailable":2,"numberMisscheduled":1}}]}"#;
    const DAEMON_SETS: &str = "/apis/apps/v1/daemonsets";
    const JOB_BODY: &str = r#"{"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"api","namespace":"shop","ownerReferences":[{"apiVersion":"batch/v1","kind":"CronJob","name":"nightly","uid":"c","controller":true}]},"spec":{"completions":2,"selector":{"matchLabels":{"app":"api"}},"template":{}},"status":{"active":1,"succeeded":1}}"#;
    const JOB_LIST_BODY: &str = r#"{"kind":"JobList","apiVersion":"batch/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api","resourceVersion":"1","ownerReferences":[{"apiVersion":"batch/v1","kind":"CronJob","name":"nightly","uid":"c","controller":true}]},"spec":{"completions":2,"selector":{"matchLabels":{"app":"api"}},"template":{}},"status":{"startTime":"2026-01-01T00:00:00Z","active":1,"succeeded":1}}]}"#;
    const JOBS: &str = "/apis/batch/v1/jobs";

    fn serve_pods(pod_list: &str) -> String {
        serve_pods_with_logs(pod_list, LOG_BODY)
    }

    fn serve_pods_with_logs(pod_list: &str, logs: &str) -> String {
        serve(Fixture {
            pod_list: pod_list.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: logs.into(),
            live_logs: None,
            forbidden: Vec::new(),
            reconnect_logs: None,
            objects: Vec::new(),
        })
    }

    fn serve_workloads() -> String {
        serve(Fixture {
            pod_list: OWNED_POD_LIST_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden: Vec::new(),
            reconnect_logs: None,
            objects: vec![
                (
                    "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c".into(),
                    REPLICA_SET_BODY.into(),
                ),
                (
                    "/apis/apps/v1/namespaces/shop/deployments/api".into(),
                    DEPLOYMENT_BODY.into(),
                ),
            ],
        })
    }

    fn serve_api() -> String {
        serve_pods(POD_LIST_BODY)
    }

    /// Pods owned by the `api` Deployment and one without a workload, and the Deployment.
    /// `forbidden` lists paths the API refuses.
    fn serve_deployments(forbidden: Vec<&'static str>) -> String {
        serve(Fixture {
            pod_list: OWNED_POD_LIST_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (DEPLOYMENTS.into(), DEPLOYMENT_LIST_BODY.into()),
                (
                    "/apis/apps/v1/namespaces/shop/deployments".into(),
                    DEPLOYMENT_LIST_BODY.into(),
                ),
                (
                    "/apis/apps/v1/namespaces/shop/deployments/api".into(),
                    DEPLOYMENT_BODY.into(),
                ),
            ],
        })
    }

    /// Pods owned by an `api` workload and one without, and the workload `api` of the kind
    /// at `/apis/{group_version}/{resource}`, listed by `list` and read as `body`.
    /// `forbidden` lists paths the API refuses.
    fn serve_workload(
        group_version: &str,
        resource: &str,
        list: &str,
        body: &str,
        forbidden: Vec<&'static str>,
    ) -> String {
        let namespaced = format!("/apis/{group_version}/namespaces/shop/{resource}");
        serve(Fixture {
            pod_list: OWNED_POD_LIST_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (format!("/apis/{group_version}/{resource}"), list.into()),
                (namespaced.clone(), list.into()),
                (format!("{namespaced}/api"), body.into()),
            ],
        })
    }

    fn init(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::init(cx);
        });
    }

    fn open_with(
        backend: Arc<Backend>,
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        options: WindowOptions,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Root::new(backend, contexts, SavedState::default(), window, cx))
            })
            .unwrap()
        })
    }

    fn open_on(
        backend: Arc<Backend>,
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        open_with(backend, contexts, WindowOptions::default(), cx)
    }

    fn open_root(
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        init(cx);
        open_on(Arc::new(Backend::new().unwrap()), contexts, cx)
    }

    fn open_sized(
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        width: f32,
        cx: &mut TestAppContext,
    ) -> AnyWindowHandle {
        init(cx);
        let bounds = Bounds::new(Point::default(), size(px(width), px(600.)));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        };
        open_with(Arc::new(Backend::new().unwrap()), contexts, options, cx).0
    }

    fn open(
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        cx: &mut TestAppContext,
    ) -> AnyWindowHandle {
        open_root(contexts, cx).0
    }

    #[gpui_kit::test]
    fn shows_why_the_kubeconfig_failed_to_load(cx: &mut TestAppContext) {
        let handle = open(Contexts::from_yaml("clusters: ["), cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let status = window.find("status");
            assert!(
                status
                    .label()
                    .unwrap()
                    .starts_with("Failed to read kubeconfig")
            );
            assert!(window.try_find("context-picker").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn offers_a_reload_when_the_kubeconfig_has_no_contexts(cx: &mut TestAppContext) {
        let handle = open(Contexts::from_yaml("{}"), cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("status").label(), Some("No clusters found"));
            assert_eq!(
                window.find("notice-action").label(),
                Some("Reload contexts")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn offers_a_retry_when_the_cluster_is_unreachable(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "dev"));
        let handle = open(contexts, cx);

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("notice-action").is_some()
        })
        .await;

        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("notice-action").label(), Some("Try again"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn switches_context_from_the_title_bar(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(
            r#"
current-context: staging
clusters:
  - name: x
    cluster:
      server: http://127.0.0.1:1
contexts:
  - name: staging
    context:
      cluster: x
      user: x
  - name: production
    context:
      cluster: x
      user: x
users:
  - name: x
    user: {}
"#,
        );
        let handle = open(contexts, cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("context-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .find("status")
                .label()
                .is_some_and(|status| status.contains("production"))
        })
        .await;
    }

    #[gpui_kit::test]
    fn waits_for_a_choice_without_a_current_context(cx: &mut TestAppContext) {
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "gone"));
        let handle = open(contexts, cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("status").label(), Some("Choose a context"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn shows_the_current_contexts_pods_on_open(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            !window.find("status").label().is_some_and(|status| {
                status.starts_with("Connecting") || status.ends_with("Loading pods")
            })
        })
        .await;

        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find("status").label(),
                Some("Connected to dev, server v1.35.0. Pods: 1")
            );
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
        })
        .unwrap();
    }

    async fn wait_for_pods(handle: AnyWindowHandle, cx: &mut TestAppContext) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .find("status")
                .label()
                .is_some_and(|status| status.contains("Pods:") || status.starts_with("Failed"))
        })
        .await;
    }

    #[gpui_kit::test]
    async fn tails_logs_and_pauses_following(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            !window
                .find_all_by_label("api-1/app 2026-01-01T00:00:00Z first line")
                .is_empty()
        })
        .await;

        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.find("log-mode").label(), Some("Pause"));
            window.click("log-mode", cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Follow"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn log_lines_show_their_severity_and_json_message(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_pods_with_logs(POD_LIST_BODY, SEVERITY_LOG_BODY);
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find(("log-line", 2u64)).is_some()
        })
        .await;

        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find(("log-line", 0u64)).label(),
                Some("api-1/app ERROR 2026-01-01T00:00:00Z ERROR payment failed")
            );
            assert_eq!(window.find(("log-severity", 0u64)).label(), Some("ERROR"));
            assert_eq!(
                window.find(("log-text", 0u64)).label(),
                Some("ERROR payment failed")
            );

            assert_eq!(
                window.find(("log-line", 1u64)).label(),
                Some(
                    r#"api-1/app WARN 2026-01-01T00:00:01Z {"level":"warn","ts":1.7e9,"msg":"slow query","ms":1204}"#
                ),
                "the raw line stays available"
            );
            assert_eq!(window.find(("log-severity", 1u64)).label(), Some("WARN"));
            assert_eq!(
                window.find(("log-text", 1u64)).label(),
                Some("slow query ms=1204")
            );

            assert_eq!(window.find(("log-severity", 2u64)).label(), Some("DEBUG"));
            assert_eq!(
                window.find(("log-text", 2u64)).label(),
                Some(r#"{"level":"debug","seq":3}"#),
                "a JSON line without a message shows as written"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn lines_lost_across_a_reconnect_show_as_a_marked_gap(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve(Fixture {
            pod_list: POD_LIST_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: "2026-01-01T00:00:00Z first\n2026-01-01T00:00:06Z second\n".into(),
            live_logs: None,
            forbidden: Vec::new(),
            objects: Vec::new(),
            reconnect_logs: Some("2026-01-01T00:00:31Z after\n".into()),
        });
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;
        let gap = Gap {
            from: 1_767_225_606_000_000_000,
            to: 1_767_225_631_000_000_000,
        };
        let label = format!("api-1/app {}", gap_text(gap, &chrono::Local));

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        // The timeline catches up with the lines at its next count.
        let timeline_marks_gap = |window: &gpui_kit::Window| {
            (0..40usize)
                .filter_map(|index| window.try_find(("log-timeline-bar", index)))
                .filter_map(|bar| bar.label().map(str::to_owned))
                .any(|label| label.ends_with("no lines, lines may be missing"))
        };
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            !window.find_all_by_label(&label).is_empty() && timeline_marks_gap(window)
        })
        .await;

        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window
                    .find_all_by_label("api-1/app 2026-01-01T00:00:31Z after")
                    .len(),
                1
            );
        })
        .unwrap();
    }

    async fn tail_severity_lines(cx: &mut TestAppContext) -> AnyWindowHandle {
        cx.executor().allow_parking();
        let server = serve_pods_with_logs(POD_LIST_BODY, SEVERITY_LOG_BODY);
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find(("log-line", 2u64)).is_some()
        })
        .await;
        handle
    }

    const ERROR_LINE: &str = "api-1/app ERROR 2026-01-01T00:00:00Z ERROR payment failed";
    const WARN_LINE: &str = r#"api-1/app WARN 2026-01-01T00:00:01Z {"level":"warn","ts":1.7e9,"msg":"slow query","ms":1204}"#;
    const DEBUG_LINE: &str = r#"api-1/app DEBUG 2026-01-01T00:00:02Z {"level":"debug","seq":3}"#;

    /// The fake server's tail reconnects and sends its lines again, so tests look at which
    /// lines show rather than how many.
    fn shows(window: &gpui_kit::Window, line: &str) -> bool {
        !window.find_all_by_label(line).is_empty()
    }

    /// The `matches of lines` count, once every stored line has been checked.
    fn finished_count(window: &gpui_kit::Window) -> Option<(u64, u64)> {
        let label = window.try_find("log-filter-count")?.label()?.to_owned();
        let (matches, lines) = label.strip_suffix(" lines")?.split_once(" of ")?;
        Some((
            matches.replace(',', "").parse().ok()?,
            lines.replace(',', "").parse().ok()?,
        ))
    }

    async fn wait_for_filter(
        handle: AnyWindowHandle,
        cx: &mut TestAppContext,
        done: impl Fn(&gpui_kit::Window, (u64, u64)) -> bool,
    ) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            finished_count(window).is_some_and(|count| done(window, count))
        })
        .await;
    }

    #[gpui_kit::test]
    async fn the_f_key_filters_log_lines_across_the_tail(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            assert!(window.try_find("log-filter-count").is_none());
            window.press("f", cx);
            window.input("PAYMENT", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |window, (matches, lines)| {
            matches > 0 && matches < lines && shows(window, ERROR_LINE) && !shows(window, WARN_LINE)
        })
        .await;

        cx.update_window(handle, |_, window, cx| window.press("escape", cx))
            .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("log-filter-count").is_none() && shows(window, WARN_LINE)
        })
        .await;
    }

    #[gpui_kit::test]
    async fn a_json_field_and_a_severity_narrow_the_lines(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.click("log-filter", cx);
            window.input("ms=1204", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |window, _| {
            shows(window, WARN_LINE) && !shows(window, ERROR_LINE) && !shows(window, DEBUG_LINE)
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.press("escape", cx);
            window.render_frame(cx);
            window.press("escape", cx);
            window.render_frame(cx);
            window.within("log-severity-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |window, _| {
            shows(window, WARN_LINE) && shows(window, ERROR_LINE) && !shows(window, DEBUG_LINE)
        })
        .await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find("log-severity-picker").value(),
                Some("WARN and above")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_filter_with_no_matches_offers_to_clear_it(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.press("f", cx);
            window.input("timeout", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |window, (matches, _)| {
            matches == 0 && window.try_find("log-notice").is_some()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                window.find("log-notice").label(),
                Some("No lines match \"timeout\"")
            );
            window.click("log-clear-filter", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("log-notice").is_none()
                && window.try_find("log-filter-count").is_none()
                && shows(window, DEBUG_LINE)
        })
        .await;
    }

    #[gpui_kit::test]
    async fn an_invalid_regular_expression_says_so(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.press("f", cx);
            window.input("/(payment/", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .try_find("log-filter-count")
                .and_then(|count| count.label().map(str::to_owned))
                == Some("/(payment/ is not a valid regular expression".to_owned())
        })
        .await;
    }

    #[gpui_kit::test]
    async fn a_filtered_tail_still_pauses_and_follows(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.press("f", cx);
            window.input("/slow|payment/", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |window, _| {
            shows(window, WARN_LINE) && shows(window, ERROR_LINE) && !shows(window, DEBUG_LINE)
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Pause"));
            window.press("space", cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Follow"));
            assert!(shows(window, ERROR_LINE) && !shows(window, DEBUG_LINE));
            window.press("space", cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Pause"));
        })
        .unwrap();
    }

    /// 200 lines a second apart, with errors at lines 20 and 120.
    fn timeline_log_body() -> String {
        (0..200)
            .map(|line| {
                let text = match line {
                    20 => "ERROR payment failed".to_owned(),
                    120 => "ERROR disk full".to_owned(),
                    line => format!("INFO ok {line}"),
                };
                format!("2026-01-01T00:{:02}:{:02}Z {text}\n", line / 60, line % 60)
            })
            .collect()
    }

    async fn tail_timeline_lines(cx: &mut TestAppContext) -> AnyWindowHandle {
        cx.executor().allow_parking();
        let server = serve_pods_with_logs(POD_LIST_BODY, &timeline_log_body());
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find(("log-line", 199u64)).is_some()
        })
        .await;
        handle
    }

    fn marked(window: &gpui_kit::Window, seq: u64) -> bool {
        window
            .try_find(("log-line", seq))
            .is_some_and(|line| line.selected() == Some(true))
    }

    fn bar(window: &gpui_kit::Window, index: usize) -> gpui_kit::test::ElementSnapshot {
        window.find(("log-timeline-bar", index))
    }

    #[gpui_kit::test]
    async fn the_timeline_stacks_lines_by_severity(cx: &mut TestAppContext) {
        let handle = tail_severity_lines(cx).await;

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find(("log-timeline-bar", 2usize)).is_some()
        })
        .await;
        cx.update_window(handle, |_, window, _| {
            let label = |index| bar(window, index).label().unwrap_or_default().to_owned();
            assert!(label(0).ends_with(": 1 ERROR"), "{}", label(0));
            assert!(label(1).ends_with(": 1 WARN"), "{}", label(1));
            assert!(label(2).ends_with(": 1 OTHER"), "{}", label(2));
            assert!(
                window
                    .find("log-timeline")
                    .label()
                    .is_some_and(|label| label.ends_with(", 1 s per bar"))
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn clicking_a_bar_shows_its_lines_and_pauses(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            assert!(window.try_find(("log-line", 0u64)).is_none());
            window.click(("log-timeline-bar", 0usize), cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Follow"));
            assert!(window.try_find(("log-line", 0u64)).is_some());
            assert_eq!(bar(window, 0).selected(), Some(true));
            assert_eq!(window.find("log-timeline").focused(), Some(true));

            window.press("right", cx);
            window.render_frame(cx);
            assert_eq!(bar(window, 1).selected(), Some(true));
            assert_eq!(bar(window, 0).selected(), Some(false));
            window.press("left", cx);
            window.render_frame(cx);
            assert_eq!(bar(window, 0).selected(), Some(true));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn n_and_shift_n_move_between_errors(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.press("shift-n", cx);
            window.render_frame(cx);
            assert!(marked(window, 120));
            assert_eq!(window.find("log-mode").label(), Some("Follow"));

            window.press("shift-n", cx);
            window.render_frame(cx);
            assert!(marked(window, 20));
            assert!(window.try_find(("log-line", 120u64)).is_none());

            window.press("n", cx);
            window.render_frame(cx);
            assert!(marked(window, 120));

            window.press("n", cx);
            window.render_frame(cx);
            assert!(marked(window, 120));
            assert_eq!(window.find("log-summary").label(), Some("No later errors"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_next_error_respects_the_filter(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.press("f", cx);
            window.input("payment", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |_, (matches, _)| matches == 1).await;
        cx.update_window(handle, |_, window, cx| {
            window.press("shift-n", cx);
            window.render_frame(cx);
            assert!(marked(window, 20));
            window.press("shift-n", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("log-summary").label(),
                Some("No earlier errors match the filter")
            );
        })
        .unwrap();
    }

    fn selected(window: &gpui_kit::Window, seq: u64) -> bool {
        marked(window, seq)
    }

    #[gpui_kit::test]
    async fn clicked_lines_copy_as_stored_with_their_source(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.click(("log-line", 195u64), cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Follow"));
            window.click_with_modifiers(("log-line", 197u64), Modifiers::shift(), cx);
            window.render_frame(cx);
            assert!(selected(window, 195) && selected(window, 196) && selected(window, 197));
            assert!(!selected(window, 198));
            window.press(super::COPY_KEY, cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-summary").label(), Some("Copied 3 lines"));
        })
        .unwrap();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(
                "[pod/api-1/app] 2026-01-01T00:03:15Z INFO ok 195\n\
                 [pod/api-1/app] 2026-01-01T00:03:16Z INFO ok 196\n\
                 [pod/api-1/app] 2026-01-01T00:03:17Z INFO ok 197"
                    .to_owned()
            )
        );
    }

    #[gpui_kit::test]
    async fn arrow_keys_move_and_extend_the_selection(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.click(("log-line", 190u64), cx);
            window.press("down", cx);
            window.render_frame(cx);
            assert!(selected(window, 191) && !selected(window, 190));
            window.press("shift-down", cx);
            window.press("shift-down", cx);
            window.render_frame(cx);
            assert!(selected(window, 191) && selected(window, 193));
            window.press("shift-up", cx);
            window.render_frame(cx);
            assert!(selected(window, 192) && !selected(window, 193));
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(!selected(window, 191));
        })
        .unwrap();
    }

    fn description(window: &gpui_kit::Window, seq: u64) -> Option<String> {
        window
            .try_find(("log-line", seq))?
            .description()
            .map(str::to_owned)
    }

    #[gpui_kit::test]
    async fn context_shows_lines_of_the_source_around_each_match(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;

        cx.update_window(handle, |_, window, cx| {
            assert!(window.try_find("log-context-picker").is_none());
            window.press("f", cx);
            window.input("error", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |_, (matches, _)| matches == 2).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("log-context-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.find("log-context-picker").value() == Some("Context: 3 lines")
                && description(window, 120).is_some()
        })
        .await;
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(description(window, 120).as_deref(), Some("Match"));
            assert_eq!(description(window, 117).as_deref(), Some("Around a match"));
            assert_eq!(description(window, 123).as_deref(), Some("Around a match"));
            assert!(window.try_find(("log-line", 124u64)).is_none());

            window.press("shift-n", cx);
            window.press("shift-n", cx);
            window.render_frame(cx);
            assert!(selected(window, 20));
            assert_eq!(description(window, 17).as_deref(), Some("Around a match"));
            assert!(window.try_find(("log-line", 16u64)).is_none());
            assert!(window.try_find(("log-separator", 120u64)).is_some());
        })
        .unwrap();
    }

    async fn export_to(
        handle: AnyWindowHandle,
        path: std::path::PathBuf,
        cx: &mut TestAppContext,
    ) -> String {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("log-export", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(move |_| Some(path));
        let mut summary = String::new();
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            summary = window
                .find("log-summary")
                .label()
                .unwrap_or_default()
                .to_owned();
            summary.starts_with("Exported ")
        })
        .await;
        summary
    }

    #[gpui_kit::test]
    async fn exports_every_stored_line_to_the_chosen_file(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("all.log");

        let summary = export_to(handle, path.clone(), cx).await;

        assert!(summary.ends_with(" lines to all.log"), "{summary}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("[pod/api-1/app] 2026-01-01T00:00:00Z INFO ok 0\n"));
        assert!(written.contains("[pod/api-1/app] 2026-01-01T00:03:19Z INFO ok 199\n"));
    }

    #[gpui_kit::test]
    async fn exports_only_what_the_filter_shows(cx: &mut TestAppContext) {
        let handle = tail_timeline_lines(cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.press("f", cx);
            window.input("error", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_for_filter(handle, cx, |_, (matches, _)| matches == 2).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("errors.log");

        let summary = export_to(handle, path.clone(), cx).await;

        assert_eq!(summary, "Exported 2 lines to errors.log");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[pod/api-1/app] 2026-01-01T00:00:20Z ERROR payment failed\n\
             [pod/api-1/app] 2026-01-01T00:02:00Z ERROR disk full\n"
        );
    }

    async fn wait_for_log_summary(handle: AnyWindowHandle, prefix: &str, cx: &mut TestAppContext) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .try_find("log-summary")
                .and_then(|summary| summary.label().map(str::to_owned))
                .is_some_and(|summary| summary.starts_with(prefix))
        })
        .await;
    }

    #[gpui_kit::test]
    async fn the_w_key_follows_the_selected_pods_deployment(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_workloads(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("j", cx);
            window.render_frame(cx);
            assert_eq!(window.find("details-title").label(), Some("api-1"));
            window.press("w", cx);
        })
        .unwrap();

        wait_for_log_summary(handle, "Following deployment/api · 1 pod · ", cx).await;
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            !window
                .find_all_by_label("api-1/app 2026-01-01T00:00:00Z first line")
                .is_empty()
        })
        .await;
    }

    #[gpui_kit::test]
    async fn the_palette_tails_a_typed_label_selector(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_workloads(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            window.input("app=web", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find_all_by_label("Tail pods matching app=web").len(),
                1
            );
            window.press("enter", cx);
        })
        .unwrap();

        wait_for_log_summary(handle, "Following app=web · 1 pod · ", cx).await;
    }

    #[gpui_kit::test]
    async fn a_pod_without_a_workload_says_so_in_the_log_dock(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("w", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("log-summary").label(),
                Some("Select a pod to tail its workload")
            );
            window.press("j", cx);
            window.press("w", cx);
        })
        .unwrap();

        wait_for_log_summary(handle, "api-1 has no workload to follow", cx).await;
    }

    const REPLICA_SET_PATH: &str = "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c";

    /// Pods and their logs in `shop`, refusing `forbidden` and anything else the role lacks.
    fn serve_limited(forbidden: Vec<&'static str>) -> String {
        serve(Fixture {
            pod_list: OWNED_POD_LIST_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![(
                "/api/v1/namespaces/shop/pods".into(),
                OWNED_POD_LIST_BODY.into(),
            )],
        })
    }

    /// A kubeconfig whose context `dev` names `namespace`.
    fn kubeconfig_in(server: &str, namespace: &str) -> String {
        kubeconfig(server, "dev").replacen(
            "      user: dev\n",
            &format!("      user: dev\n      namespace: {namespace}\n"),
            1,
        )
    }

    #[gpui_kit::test]
    async fn a_refused_workload_says_why_in_the_log_dock(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_limited(vec![REPLICA_SET_PATH]);
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;

        press(handle, "j", cx);
        press(handle, "w", cx);

        wait_for_log_summary(
            handle,
            r#"Can't read replicaset/api-7f9c to find its pods. Press l to tail this pod alone. The API says: replicasets.apps "api-7f9c" is forbidden: User "jane" cannot get resource "replicasets" in API group "apps" in the namespace "shop""#,
            cx,
        )
        .await;
    }

    #[gpui_kit::test]
    async fn refused_events_say_the_apis_reason(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_limited(vec!["/api/v1/namespaces/shop/events"]);
        let handle = open(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;

        press(handle, "j", cx);
        press(handle, "e", cx);

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("details-message").is_some_and(|message| {
                message.label()
                    == Some(
                        r#"events is forbidden: User "jane" cannot list resource "events" in API group "" in the namespace "shop""#,
                    )
            })
        })
        .await;
    }

    #[gpui_kit::test]
    async fn a_refused_cluster_pod_list_shows_the_contexts_namespace(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_limited(vec!["/api/v1/pods"]);
        let contexts = Contexts::from_yaml(&kubeconfig_in(&server, "shop"));
        let (handle, root) = open_root(contexts, cx);

        wait_for_label(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("context-namespace-note").label(),
                Some(
                    "Showing shop, the namespace your context names, because you can't list pods across all namespaces."
                )
            );
            assert!(window.try_find("notice-title").is_none());
            let state = root.read(cx).saved_state(window, cx);
            assert_eq!(state.namespace, None);
        })
        .unwrap();
        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx).as_deref(), Some("shop"));
        });

        cx.update_window(handle, |_, window, cx| {
            root.update(cx, |root, cx| root.set_namespace(None, window, cx));
            window.render_frame(cx);
        })
        .unwrap();
        wait_for_notice(handle, "You can't list Pods across all namespaces", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("context-namespace-note").is_none());
            assert_eq!(
                window.find("notice-action").label(),
                Some("Show namespace shop")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_context_without_a_namespace_keeps_the_no_permission_state(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_limited(vec!["/api/v1/pods"]);
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);

        wait_for_notice(handle, "You can't list Pods across all namespaces", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("context-namespace-note").is_none());
            assert_eq!(
                window.find("notice-action").label(),
                Some("Show namespace default")
            );
        })
        .unwrap();
        root.read_with(cx, |root, cx| assert_eq!(root.namespace(cx), None));
    }

    #[gpui_kit::test]
    async fn a_user_who_can_list_across_namespaces_stays_on_all_of_them(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let server = serve_limited(Vec::new());
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig_in(&server, "shop")), cx);

        wait_for_pods(handle, cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("context-namespace-note").is_none());
        })
        .unwrap();
        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx), None);
            assert_eq!(root.context_namespace, ContextNamespace::Off);
        });
    }

    #[gpui_kit::test]
    async fn closing_a_window_drops_its_session_and_leaves_the_other_running(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        init(cx);
        let backend = Arc::new(Backend::new().unwrap());
        let server = serve_api();
        let contexts = || Contexts::from_yaml(&kubeconfig(&server, "dev"));
        let (closing, root) = open_on(backend.clone(), contexts(), cx);
        let (kept, _kept_root) = open_on(backend.clone(), contexts(), cx);
        wait_for_pods(closing, cx).await;
        wait_for_pods(kept, cx).await;
        cx.update_window(closing, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        let logs = cx.read_entity(&root, |root, _| root.logs.as_ref().unwrap().downgrade());
        let closed_root = root.downgrade();
        drop(root);

        cx.update_window(closing, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();

        assert!(closed_root.upgrade().is_none());
        assert!(logs.upgrade().is_none());
        // This test, then the kept window's Root.
        assert_eq!(Arc::strong_count(&backend), 2);
        cx.update_window(kept, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("status").label(),
                Some("Connected to dev, server v1.35.0. Pods: 1")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn clicking_a_row_opens_its_details_and_escape_closes_them(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("details").is_none());
            window.click(("row", 0usize), cx);
        })
        .unwrap();

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("copy-yaml").is_none()
                && window.find("details-title").label() == Some("api-1")
                && !window.find_all_by_label("Events 1").is_empty()
        })
        .await;

        cx.update_window(handle, |_, window, cx| {
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("details").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn typing_in_the_filter_does_not_move_the_selection(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("pod-filter", cx);
            window.input("j", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("details").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_j_key_selects_the_first_pod(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("j", cx);
            window.render_frame(cx);
            assert_eq!(window.find("details-title").label(), Some("api-1"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn restores_the_saved_namespace_and_hidden_columns(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_pods(TWO_NAMESPACES_BODY), "dev"));
        init(cx);
        let saved = SavedState {
            namespace: Some("web".into()),
            hidden_columns: vec!["Node".into()],
            ..SavedState::default()
        };
        let backend = Arc::new(Backend::new().unwrap());
        let (handle, root) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Root::new(backend, contexts, saved, window, cx))
            })
            .unwrap()
        });
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
            assert!(window.find_all_by_label("api-1").is_empty());
            let state = root.read(cx).saved_state(window, cx);
            assert_eq!(state.namespace.as_deref(), Some("web"));
            assert_eq!(state.hidden_columns, ["Node"]);
        })
        .unwrap();
    }

    async fn connected(cx: &mut TestAppContext) -> AnyWindowHandle {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_pods(TWO_NAMESPACES_BODY), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;
        handle
    }

    #[gpui_kit::test]
    async fn the_palette_finds_a_pod_and_opens_it(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            window.input("front", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_none());
            assert_eq!(window.find("details-title").label(), Some("front-1"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn escape_leaves_the_palette_without_acting(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_some());
            window.input("nothing", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_some());
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_none());
            assert!(window.try_find("details").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_pod_is_reachable_from_the_keyboard_alone(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("/", cx);
            window.input("front", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("details-title").is_some()
                && !window.find_all_by_label("Events 1").is_empty()
        })
        .await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("y", cx);
            window.render_frame(cx);
            assert!(window.try_find("copy-yaml").is_some());
            window.press("e", cx);
            window.render_frame(cx);
            assert!(window.try_find("copy-yaml").is_none());
            window.press("l", cx);
        })
        .unwrap();

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find("log-mode").is_some()
        })
        .await;

        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.find("log-mode").label(), Some("Pause"));
            window.press("space", cx);
            window.render_frame(cx);
            assert_eq!(window.find("log-mode").label(), Some("Follow"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn escape_clears_the_filter_and_then_closes_the_panel(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("j", cx);
            window.render_frame(cx);
            window.press("/", cx);
            window.input("front", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find_all_by_label("api-1").len() <= 1);
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("details").is_some());
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
            assert!(!window.find_all_by_label("api-1").is_empty());
            window.press("escape", cx);
            window.render_frame(cx);
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("details").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_question_mark_lists_every_shortcut(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("?", cx);
            window.render_frame(cx);
            assert!(window.try_find("shortcuts").is_some());
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("shortcuts").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_palette_opens_about_with_the_version(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            window.input("about", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
            assert_eq!(window.find("about-version").label(), Some(version.as_str()));
            assert!(window.try_find("about-repository").is_some());
            assert!(window.try_find("about-notices").is_some());
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("about").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn filtering_narrows_the_table_and_the_tail(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
            assert_eq!(window.find("tail-logs").label(), Some("Tail 1 pod"));
            window.click("pod-filter", cx);
            window.input("web", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find_all_by_label("api-1").is_empty());
            assert_eq!(window.find("tail-logs").label(), Some("Tail logs"));
            assert!(window.find("notice-action").label() == Some("Clear filter"));
            window.click("notice-action", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
        })
        .unwrap();
    }

    async fn open_two_namespaces(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_pods(TWO_NAMESPACES_BODY), "dev"));
        let (handle, root) = open_root(contexts, cx);
        wait_for_pods(handle, cx).await;
        (handle, root)
    }

    fn set_namespace(
        handle: AnyWindowHandle,
        root: &Entity<Root>,
        namespace: &str,
        cx: &mut TestAppContext,
    ) {
        cx.update_window(handle, |_, window, cx| {
            root.update(cx, |root, cx| {
                root.set_namespace(Some(namespace.into()), window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn choosing_a_namespace_narrows_the_table_and_the_tail(cx: &mut TestAppContext) {
        let (handle, _) = open_two_namespaces(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("namespace-picker").value(),
                Some("All namespaces")
            );
            assert_eq!(window.find("tail-logs").label(), Some("Tail 2 pods"));
            window.within("namespace-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("namespace-picker").value(), Some("shop"));
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
            assert!(window.find_all_by_label("front-1").is_empty());
            assert_eq!(window.find("tail-logs").label(), Some("Tail 1 pod"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn typing_in_the_namespace_picker_searches_it(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("namespace-picker").click("input", cx);
            window.render_frame(cx);
            window.input("web", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();

        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx).as_deref(), Some("web"));
        });
    }

    #[gpui_kit::test]
    async fn a_snapshot_with_the_same_namespaces_leaves_a_search_alone(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("namespace-picker").click("input", cx);
            window.render_frame(cx);
            window.input("web", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            root.update(cx, |root, cx| {
                let snapshot = root.pods.table.read(cx).delegate().snapshot().clone();
                root.show_pods(Arc::new(snapshot), window, cx);
            });
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();

        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx).as_deref(), Some("web"));
        });
    }

    #[gpui_kit::test]
    async fn show_all_namespaces_returns_from_an_empty_namespace(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;
        set_namespace(handle, &root, "payments", cx);

        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.find("namespace-picker").value(), Some("payments"));
            assert_eq!(window.find("tail-logs").label(), Some("Tail logs"));
            assert_eq!(
                window.find("notice-action").label(),
                Some("Show all namespaces")
            );
            window.click("notice-action", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("namespace-picker").value(),
                Some("All namespaces")
            );
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
        })
        .unwrap();
        root.read_with(cx, |root, cx| assert_eq!(root.namespace(cx), None));
    }

    #[gpui_kit::test]
    async fn a_namespace_that_disappears_stays_chosen(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;
        set_namespace(handle, &root, "shop", cx);

        cx.update_window(handle, |_, window, cx| {
            let snapshot = Arc::new(PodSnapshot {
                synced: true,
                ..PodSnapshot::default()
            });
            root.update(cx, |root, cx| root.show_pods(snapshot, window, cx));
            window.render_frame(cx);
            assert_eq!(window.find("namespace-picker").value(), Some("shop"));
            assert_eq!(
                window.find("notice-action").label(),
                Some("Show all namespaces")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_namespace_hiding_the_selected_pod_keeps_its_details_open(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("j", cx);
            window.render_frame(cx);
            assert_eq!(window.find("details-title").label(), Some("api-1"));
        })
        .unwrap();
        set_namespace(handle, &root, "web", cx);

        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("front-1").len() == 1);
            assert_eq!(window.find("details-title").label(), Some("api-1"));
        })
        .unwrap();
    }

    fn headers(root: &Entity<Root>, cx: &mut TestAppContext) -> Vec<String> {
        root.read_with(cx, |root, cx| root.pods.table.read(cx).headers(cx))
    }

    fn first_row(root: &Entity<Root>, cx: &mut TestAppContext) -> Vec<String> {
        root.read_with(cx, |root, cx| {
            let table = root.pods.table.read(cx).delegate();
            (0..table.columns_count(cx))
                .filter_map(|position| table.cell_label(0, position, cx))
                .map(|cell| cell.to_string())
                .collect()
        })
    }

    fn open_columns_menu(handle: AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("columns", cx);
            window.render_frame(cx);
        })
        .unwrap();
    }

    fn click_menu_item(handle: AnyWindowHandle, label: &str, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            let item = window.within("popup-menu").find_by_label(label);
            let id = item.path().last().unwrap().clone();
            window.within("popup-menu").click(id, cx);
            window.render_frame(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_columns_menu_hides_and_shows_a_column(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;

        open_columns_menu(handle, cx);
        click_menu_item(handle, "Namespace", cx);

        assert_eq!(
            headers(&root, cx),
            ["Name", "Status", "Node", "Ready", "Restarts", "Age"]
        );
        assert_eq!(first_row(&root, cx)[..3], ["api-1", "Running", "node-1"]);
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find_by_label("Sort by Namespace").is_none());
            assert!(window.try_find_by_label("Sort by Status").is_some());
            assert!(window.find_all_by_label("shop").is_empty());
        })
        .unwrap();

        open_columns_menu(handle, cx);
        cx.update_window(handle, |_, window, _| {
            let item = window.within("popup-menu").find_by_label("Namespace");
            assert_eq!(item.checked(), Some(false));
        })
        .unwrap();
        click_menu_item(handle, "Namespace", cx);

        assert_eq!(headers(&root, cx).len(), 7);
        root.read_with(cx, |root, cx| assert!(root.hidden_columns(cx).is_empty()));
    }

    #[gpui_kit::test]
    async fn the_columns_menu_keeps_name_shown(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;

        open_columns_menu(handle, cx);
        cx.update_window(handle, |_, window, _| {
            let name = window.within("popup-menu").find_by_label("Name");
            assert_eq!(name.checked(), Some(true));
        })
        .unwrap();
        click_menu_item(handle, "Name", cx);

        assert_eq!(headers(&root, cx).len(), 7);
        root.read_with(cx, |root, cx| assert!(root.hidden_columns(cx).is_empty()));
    }

    #[gpui_kit::test]
    async fn a_header_sorts_its_own_column_after_another_is_hidden(cx: &mut TestAppContext) {
        let (handle, root) = open_two_namespaces(cx).await;
        root.update(cx, |root, cx| root.set_hidden_columns(["Namespace"], cx));

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let header = window.find_by_label("Sort by Name");
            let id = header.path().last().unwrap().clone();
            window.click(id.clone(), cx);
            window.render_frame(cx);
            window.click(id, cx);
            window.render_frame(cx);
        })
        .unwrap();

        assert_eq!(first_row(&root, cx)[0], "front-1");
    }

    #[gpui_kit::test]
    async fn the_header_fits_at_the_minimum_window_width(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_pods(TWO_NAMESPACES_BODY), "dev"));
        let handle = open_sized(contexts, 760., cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let picker = window.find("namespace-picker").bounds();
            let filter = window.find("pod-filter").bounds();
            let columns = window.find("columns").bounds();
            let tail = window.find("tail-logs").bounds();
            assert!(picker.right() <= filter.left());
            assert!(filter.right() <= columns.left());
            assert!(columns.right() <= tail.left());
            assert!(tail.right() <= window.viewport_size().width);
            assert!(filter.size.width >= px(96.));
            assert_eq!(window.find("columns").label(), Some("Columns"));
        })
        .unwrap();
    }

    fn pod_in(namespace: &str) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: namespace.into(),
            name: "api-1".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            node: "node-1".into(),
            containers: vec!["app".into()],
            ..PodRow::default()
        })
    }

    fn table_of(namespaces: &[&str]) -> PodTable {
        let mut table = PodTable::default();
        table.show(Arc::new(PodSnapshot {
            rows: namespaces
                .iter()
                .map(|namespace| pod_in(namespace))
                .collect(),
            synced: true,
            error: None,
        }));
        table
    }

    fn notice_for(table: &PodTable) -> (String, Option<SharedString>) {
        let notice = no_rows_notice(table);
        (
            notice.title.to_string(),
            notice.action.map(|(_, label)| label),
        )
    }

    #[test]
    fn an_empty_namespace_offers_all_namespaces() {
        let mut table = table_of(&["shop"]);
        table.set_namespace(Some("payments".into()));
        assert_eq!(
            notice_for(&table),
            (
                "No pods in payments".into(),
                Some("Show all namespaces".into())
            )
        );
    }

    #[test]
    fn a_text_filter_with_no_matches_in_a_namespace_offers_to_clear_it() {
        let mut table = table_of(&["shop"]);
        table.set_namespace(Some("shop".into()));
        table.set_filter("web".into());
        assert_eq!(
            notice_for(&table),
            ("No pods match \"web\"".into(), Some("Clear filter".into()))
        );
    }

    #[test]
    fn an_empty_namespace_explains_itself_before_a_text_filter() {
        let mut table = table_of(&["shop"]);
        table.set_namespace(Some("payments".into()));
        table.set_filter("web".into());
        assert_eq!(
            notice_for(&table),
            (
                "No pods in payments".into(),
                Some("Show all namespaces".into())
            )
        );
    }

    #[test]
    fn an_empty_cluster_has_nothing_to_clear() {
        assert_eq!(
            notice_for(&table_of(&[])),
            ("No pods in this cluster".into(), None)
        );
    }

    fn target(pod: &str, container: &str) -> LogTarget {
        LogTarget {
            namespace: "shop".into(),
            pod: pod.into(),
            container: container.into(),
        }
    }

    #[test]
    fn tail_button_counts_pods_rather_than_containers() {
        let targets = [target("a", "app"), target("a", "proxy"), target("b", "app")];
        assert_eq!(distinct_pods(&targets), 2);
    }

    #[test]
    fn tail_button_says_when_the_tail_limit_applies() {
        assert_eq!(tail_label(0, 0), "Tail logs");
        assert_eq!(tail_label(1, 1), "Tail 1 pod");
        assert_eq!(tail_label(50, 50), "Tail 50 pods");
        assert_eq!(tail_label(50, 5_500), "Tail first 50 pods");
    }

    #[test]
    fn status_reports_loading_count_and_watch_errors() {
        let mut snapshot = PodSnapshot::default();
        assert_eq!(pod_status("Up", &snapshot), "Up. Loading pods");

        snapshot.synced = true;
        assert_eq!(pod_status("Up", &snapshot), "Up. Pods: 0");

        snapshot.error = Some(WatchError::Failed {
            message: "forbidden".into(),
        });
        assert_eq!(
            pod_status("Up", &snapshot),
            "Up. Pod watch failed: forbidden"
        );
    }

    async fn open_deployments(
        forbidden: Vec<&'static str>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_deployments(forbidden), "dev"));
        let (handle, root) = open_root(contexts, cx);
        wait_for_pods(handle, cx).await;
        (handle, root)
    }

    async fn wait_for_label(handle: AnyWindowHandle, label: &str, cx: &mut TestAppContext) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            !window.find_all_by_label(label).is_empty()
        })
        .await;
    }

    async fn wait_for_notice(handle: AnyWindowHandle, title: &str, cx: &mut TestAppContext) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .try_find("notice-title")
                .is_some_and(|notice| notice.label() == Some(title))
        })
        .await;
    }

    fn press(handle: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn number_keys_and_the_palette_move_between_views(cx: &mut TestAppContext) {
        let (handle, _) = open_deployments(Vec::new(), cx).await;

        press(handle, "2", cx);
        wait_for_label(handle, "Filter deployments", cx).await;
        wait_for_label(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(window.find_all_by_label("1/1").len(), 1);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            window.input("Go to Pods", cx);
        })
        .unwrap();
        press(handle, "enter", cx);
        wait_for_label(handle, "web-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find_all_by_label("Filter pods").len(), 1);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_refused_list_shows_no_permission_for_that_view_only(cx: &mut TestAppContext) {
        let (handle, root) = open_deployments(vec![DEPLOYMENTS], cx).await;

        press(handle, "2", cx);
        wait_for_notice(
            handle,
            "You can't list Deployments across all namespaces",
            cx,
        )
        .await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find("notice-action").label(),
                Some("Show namespace default")
            );
        })
        .unwrap();
        root.read_with(cx, |root, cx| {
            let locked: Vec<_> = root
                .sidebar_items(cx)
                .into_iter()
                .filter(|item| item.forbidden)
                .map(|item| item.view)
                .collect();
            assert_eq!(locked, [View::Deployments]);
        });

        press(handle, "1", cx);
        wait_for_label(handle, "web-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find("status").label(),
                Some("Connected to dev, server v1.35.0. Pods: 2")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_refused_cluster_list_falls_back_to_the_chosen_namespace(cx: &mut TestAppContext) {
        let (handle, root) = open_deployments(vec![DEPLOYMENTS], cx).await;
        set_namespace(handle, &root, "shop", cx);

        press(handle, "2", cx);
        wait_for_label(handle, "api", cx).await;

        set_namespace(handle, &root, "web", cx);
        wait_for_notice(handle, "No deployments in web", cx).await;
    }

    #[gpui_kit::test]
    async fn a_deployment_opens_its_details_and_shows_its_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_deployments(Vec::new(), cx).await;
        press(handle, "2", cx);
        wait_for_label(handle, "api", cx).await;

        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details-title").label(), Some("api"));
            assert_eq!(window.find("details").label(), Some("Deployment details"));
            assert_eq!(window.find("show-its-pods").label(), Some("Show its pods"));
        })
        .unwrap();

        press(handle, "p", cx);
        wait_for_label(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, cx| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(
                window.find("selector-filter").label(),
                Some("Showing pods of deployment/api")
            );
            window.click("clear-selector", cx);
        })
        .unwrap();
        wait_for_label(handle, "web-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("selector-filter").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_w_key_follows_the_selected_deployments_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_deployments(Vec::new(), cx).await;
        press(handle, "2", cx);
        wait_for_label(handle, "api", cx).await;

        press(handle, "j", cx);
        press(handle, "w", cx);

        wait_for_log_summary(handle, "Following deployment/api", cx).await;
    }

    const NODES: &str = "/api/v1/nodes";
    const NODE_LIST_BODY: &str = r#"{"kind":"NodeList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"node-1","resourceVersion":"1","labels":{"node-role.kubernetes.io/worker":""}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64","bootID":"","containerRuntimeVersion":"containerd://2.0.1","kernelVersion":"6.8.0","kubeProxyVersion":"","kubeletVersion":"v1.35.0","machineID":"","operatingSystem":"linux","osImage":"Ubuntu","systemUUID":""}}},{"metadata":{"name":"node-2","resourceVersion":"1"},"spec":{"unschedulable":true},"status":{"conditions":[{"type":"Ready","status":"False"}]}}]}"#;
    const NODE_BODY: &str = r#"{"kind":"Node","apiVersion":"v1","metadata":{"name":"node-2","resourceVersion":"1"},"spec":{"unschedulable":true},"status":{"conditions":[{"type":"Ready","status":"False"}]}}"#;
    const PODS_ON_NODES_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}},{"metadata":{"namespace":"web","name":"front-1","resourceVersion":"1"},"spec":{"nodeName":"node-2","containers":[{"name":"app"}]},"status":{"phase":"Running"}},{"metadata":{"namespace":"web","name":"front-2","resourceVersion":"1"},"spec":{"nodeName":"node-2","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;

    /// Pods on two nodes, and the nodes. `forbidden` lists paths the API refuses.
    async fn open_nodes(
        forbidden: Vec<&'static str>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let server = serve(Fixture {
            pod_list: PODS_ON_NODES_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (NODES.into(), NODE_LIST_BODY.into()),
                ("/api/v1/nodes/node-2".into(), NODE_BODY.into()),
            ],
        });
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_label(handle, "front-2", cx).await;
        press(handle, "6", cx);
        (handle, root)
    }

    #[gpui_kit::test]
    async fn the_nodes_view_lists_nodes_cluster_wide_with_their_pods(cx: &mut TestAppContext) {
        let (handle, root) = open_nodes(Vec::new(), cx).await;
        set_namespace(handle, &root, "shop", cx);

        wait_for_label(handle, "node-2", cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_label("Filter nodes").len(), 1);
            assert!(window.try_find("namespace-picker").is_none());
            assert_eq!(
                window.find("cluster-wide").label(),
                Some("Nodes are cluster-wide, so the namespace does not apply")
            );
            assert!(window.try_find("tail-logs").is_none());
            assert_eq!(window.find_all_by_label("v1.35.0").len(), 1);
        })
        .unwrap();
        let rows = root.read_with(cx, |root, cx| {
            let panel = root.panel();
            (0..2)
                .map(|row| panel.row_cells(row, cx))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            rows,
            [
                ["node-2", "NotReady, SchedulingDisabled", "", "", "2", ""],
                ["node-1", "Ready", "worker", "v1.35.0", "1", ""],
            ]
        );
    }

    #[gpui_kit::test]
    async fn show_its_pods_narrows_the_pods_to_the_node(cx: &mut TestAppContext) {
        let (handle, root) = open_nodes(Vec::new(), cx).await;
        set_namespace(handle, &root, "shop", cx);
        wait_for_label(handle, "node-2", cx).await;

        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details").label(), Some("Node details"));
            assert_eq!(window.find("details-title").label(), Some("node-2"));
        })
        .unwrap();
        press(handle, "p", cx);

        wait_for_label(handle, "front-2", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("api-1").is_empty());
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
            assert_eq!(
                window.find("selector-filter").label(),
                Some("Showing pods of node/node-2")
            );
            assert_eq!(
                window.find("namespace-picker").value(),
                Some("All namespaces")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_pods_node_link_opens_the_node(cx: &mut TestAppContext) {
        let (handle, root) = open_nodes(Vec::new(), cx).await;
        press(handle, "1", cx);
        wait_for_label(handle, "api-1", cx).await;
        set_namespace(handle, &root, "shop", cx);
        press(handle, "j", cx);
        wait_for_label(handle, "Go to node/node-1", cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.click("link node/node-1", cx);
        })
        .unwrap();

        wait_for_label(handle, "Filter nodes", cx).await;
        wait_for_details(handle, "node-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("go-back").label(), Some("Back to pod api-1"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_refused_node_list_says_so_for_the_whole_cluster(cx: &mut TestAppContext) {
        let (handle, root) = open_nodes(vec![NODES], cx).await;

        wait_for_notice(handle, "You can't list Nodes in this cluster", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("notice-action").is_none());
        })
        .unwrap();
        root.read_with(cx, |root, cx| {
            let locked: Vec<_> = root
                .sidebar_items(cx)
                .into_iter()
                .filter(|item| item.forbidden)
                .map(|item| item.view)
                .collect();
            assert_eq!(locked, [View::Nodes]);
        });

        press(handle, "1", cx);
        wait_for_label(handle, "front-2", cx).await;
    }

    const NAMESPACES: &str = "/api/v1/namespaces";
    const NAMESPACE_LIST_BODY: &str = r#"{"kind":"NamespaceList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"shop","resourceVersion":"1"},"status":{"phase":"Active"}},{"metadata":{"name":"web","resourceVersion":"1"},"status":{"phase":"Active"}},{"metadata":{"name":"old","resourceVersion":"1","deletionTimestamp":"2026-01-01T00:00:00Z"},"status":{"phase":"Terminating"}}]}"#;
    const NAMESPACE_BODY: &str = r#"{"kind":"Namespace","apiVersion":"v1","metadata":{"name":"old","resourceVersion":"1"},"status":{"phase":"Terminating"}}"#;

    /// Pods in two namespaces, and three namespaces. `forbidden` lists paths the API refuses.
    async fn open_namespaces(
        forbidden: Vec<&'static str>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let server = serve(Fixture {
            pod_list: PODS_ON_NODES_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (NAMESPACES.into(), NAMESPACE_LIST_BODY.into()),
                ("/api/v1/namespaces/old".into(), NAMESPACE_BODY.into()),
                ("/api/v1/namespaces/web".into(), NAMESPACE_BODY.into()),
                (NODES.into(), NODE_LIST_BODY.into()),
            ],
        });
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_label(handle, "front-2", cx).await;
        (handle, root)
    }

    #[gpui_kit::test]
    async fn the_namespaces_view_counts_pods_in_each(cx: &mut TestAppContext) {
        let (handle, root) = open_namespaces(Vec::new(), cx).await;
        press(handle, "7", cx);

        wait_for_label(handle, "old", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find_all_by_label("Filter namespaces").len(), 1);
            assert!(window.try_find("namespace-picker").is_none());
        })
        .unwrap();
        let rows = root.read_with(cx, |root, cx| {
            let panel = root.panel();
            (0..3)
                .map(|row| panel.row_cells(row, cx))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            rows,
            [
                ["old", "Terminating", "0", ""],
                ["shop", "Active", "1", ""],
                ["web", "Active", "2", ""],
            ]
        );
    }

    #[gpui_kit::test]
    async fn enter_on_a_namespace_chooses_it_and_returns_to_the_view_before(
        cx: &mut TestAppContext,
    ) {
        let (handle, root) = open_namespaces(Vec::new(), cx).await;
        press(handle, "7", cx);
        wait_for_label(handle, "web", cx).await;

        press(handle, "j", cx);
        press(handle, "j", cx);
        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details").label(), Some("Namespace details"));
            assert_eq!(window.find("details-title").label(), Some("web"));
            assert_eq!(
                window.find("use-namespace").label(),
                Some("Use namespace web")
            );
        })
        .unwrap();
        press(handle, "enter", cx);

        wait_for_label(handle, "Filter pods", cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("namespace-picker").value(), Some("web"));
            assert!(window.find_all_by_label("api-1").is_empty());
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
        })
        .unwrap();
        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx).as_deref(), Some("web"));
        });
    }

    #[gpui_kit::test]
    async fn the_use_namespace_button_chooses_it(cx: &mut TestAppContext) {
        let (handle, root) = open_namespaces(Vec::new(), cx).await;
        press(handle, "7", cx);
        wait_for_label(handle, "shop", cx).await;
        press(handle, "j", cx);
        press(handle, "j", cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("use-namespace", cx);
        })
        .unwrap();

        wait_for_label(handle, "api-1", cx).await;
        root.read_with(cx, |root, cx| {
            assert_eq!(root.view, View::Pods);
            assert_eq!(root.namespace(cx).as_deref(), Some("shop"));
        });
    }

    #[gpui_kit::test]
    async fn choosing_a_namespace_from_a_cluster_view_goes_to_pods(cx: &mut TestAppContext) {
        let (handle, root) = open_namespaces(Vec::new(), cx).await;
        press(handle, "6", cx);
        wait_for_label(handle, "node-2", cx).await;
        press(handle, "7", cx);
        wait_for_label(handle, "shop", cx).await;

        press(handle, "j", cx);
        press(handle, "j", cx);
        press(handle, "enter", cx);

        wait_for_label(handle, "api-1", cx).await;
        root.read_with(cx, |root, cx| {
            assert_eq!(root.view, View::Pods);
            assert_eq!(root.namespace(cx).as_deref(), Some("shop"));
        });
    }

    #[gpui_kit::test]
    async fn a_refused_namespace_list_says_so_for_the_whole_cluster(cx: &mut TestAppContext) {
        let (handle, root) = open_namespaces(vec![NAMESPACES], cx).await;
        press(handle, "7", cx);

        wait_for_notice(handle, "You can't list Namespaces in this cluster", cx).await;
        root.read_with(cx, |root, cx| {
            let locked: Vec<_> = root
                .sidebar_items(cx)
                .into_iter()
                .filter(|item| item.forbidden)
                .map(|item| item.view)
                .collect();
            assert_eq!(locked, [View::Namespaces]);
        });
    }

    const EVENTS: &str = "/apis/events.k8s.io/v1/events";
    const EVENT_LIST_BODY: &str = r#"{"kind":"EventList","apiVersion":"events.k8s.io/v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"web","name":"front-2.1","resourceVersion":"1"},"eventTime":null,"deprecatedLastTimestamp":"2026-11-07T10:00:00Z","type":"Warning","reason":"BackOff","regarding":{"apiVersion":"v1","kind":"Pod","namespace":"web","name":"front-2"},"note":"Back-off restarting failed container app"},{"metadata":{"namespace":"web","name":"front-2.2","resourceVersion":"1"},"eventTime":null,"deprecatedLastTimestamp":"2026-11-07T10:05:00Z","type":"Normal","reason":"Pulled","regarding":{"apiVersion":"v1","kind":"Pod","namespace":"web","name":"front-2"},"note":"Pulled"},{"metadata":{"namespace":"default","name":"node-2.1","resourceVersion":"1"},"eventTime":null,"deprecatedLastTimestamp":"2026-11-07T10:06:00Z","type":"Warning","reason":"NodeNotReady","regarding":{"kind":"Node","name":"node-2"},"note":"Node node-2 status is now: NodeNotReady"},{"metadata":{"namespace":"web","name":"front.1","resourceVersion":"1"},"eventTime":null,"deprecatedLastTimestamp":"2026-11-07T10:01:00Z","type":"Normal","reason":"Synced","regarding":{"apiVersion":"v1","kind":"Service","namespace":"web","name":"front"},"note":"Synced"}]}"#;
    const EVENT_BODY: &str = r#"{"kind":"Event","apiVersion":"events.k8s.io/v1","metadata":{"namespace":"web","name":"front.1","resourceVersion":"1"},"eventTime":null,"type":"Normal","reason":"Synced","regarding":{"kind":"Service","namespace":"web","name":"front"},"note":"Synced"}"#;
    const CORE_EVENT_BODY: &str = r#"{"kind":"Event","apiVersion":"v1","metadata":{"namespace":"shop","name":"e1","resourceVersion":"1"},"involvedObject":{"kind":"Pod","name":"api-1"},"type":"Warning","reason":"BackOff","message":"Back-off restarting failed container"}"#;

    /// Pods on two nodes, the nodes, and `events` at the `events.k8s.io` path, where an
    /// empty body answers 404. `forbidden` lists paths the API refuses.
    async fn open_events(
        events: &str,
        forbidden: Vec<&'static str>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let server = serve(Fixture {
            pod_list: PODS_ON_NODES_BODY.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (EVENTS.into(), events.into()),
                (
                    "/apis/events.k8s.io/v1/namespaces/web/events/front.1".into(),
                    EVENT_BODY.into(),
                ),
                (
                    "/api/v1/namespaces/shop/events/e1".into(),
                    CORE_EVENT_BODY.into(),
                ),
                (NODES.into(), NODE_LIST_BODY.into()),
                ("/api/v1/nodes/node-2".into(), NODE_BODY.into()),
            ],
        });
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_label(handle, "front-2", cx).await;
        press(handle, "8", cx);
        (handle, root)
    }

    #[gpui_kit::test]
    async fn warnings_lead_the_events_view_newest_first(cx: &mut TestAppContext) {
        let (handle, root) = open_events(EVENT_LIST_BODY, Vec::new(), cx).await;

        wait_for_label(handle, "node/node-2", cx).await;
        let rows = root.read_with(cx, |root, cx| {
            let panel = root.panel();
            (0..4)
                .map(|row| panel.row_cells(row, cx)[1..5].to_vec())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            rows,
            [
                [
                    "Warning",
                    "NodeNotReady",
                    "node/node-2",
                    "Node node-2 status is now: NodeNotReady"
                ],
                [
                    "Warning",
                    "BackOff",
                    "pod/front-2",
                    "Back-off restarting failed container app"
                ],
                ["Normal", "Pulled", "pod/front-2", "Pulled"],
                ["Normal", "Synced", "service/front", "Synced"],
            ]
        );
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find_all_by_label("Filter events").len(), 1);
            assert!(window.try_find("namespace-picker").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn enter_on_an_event_goes_to_its_object(cx: &mut TestAppContext) {
        let (handle, root) = open_events(EVENT_LIST_BODY, Vec::new(), cx).await;
        set_namespace(handle, &root, "shop", cx);
        set_namespace(handle, &root, "web", cx);
        wait_for_label(handle, "pod/front-2", cx).await;

        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details").label(), Some("Event details"));
            assert_eq!(
                window.find_by_label("Go to pod/front-2").role(),
                Some(gpui_kit::Role::Link)
            );
        })
        .unwrap();
        press(handle, "enter", cx);

        wait_for_label(handle, "Filter pods", cx).await;
        wait_for_details(handle, "front-2", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details").label(), Some("Pod details"));
            assert_eq!(
                window.find("go-back").label(),
                Some("Back to event front-2.1")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn an_event_about_a_node_opens_the_node(cx: &mut TestAppContext) {
        let (handle, root) = open_events(EVENT_LIST_BODY, Vec::new(), cx).await;
        set_namespace(handle, &root, "web", cx);
        set_namespace(handle, &root, "default", cx);
        wait_for_label(handle, "node/node-2", cx).await;

        press(handle, "j", cx);
        press(handle, "enter", cx);

        wait_for_label(handle, "Filter nodes", cx).await;
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .try_find("details-title")
                .is_some_and(|title| title.label() == Some("node-2"))
        })
        .await;
    }

    #[gpui_kit::test]
    async fn an_event_about_a_kind_without_a_view_stays(cx: &mut TestAppContext) {
        let (handle, root) = open_events(EVENT_LIST_BODY, Vec::new(), cx).await;
        wait_for_label(handle, "service/front", cx).await;

        for _ in 0..4 {
            press(handle, "j", cx);
        }
        press(handle, "enter", cx);

        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find_by_label("Go to service/front").is_none());
            assert!(window.try_find("text service/front").is_some());
            assert_eq!(window.find_all_by_label("Filter events").len(), 1);
        })
        .unwrap();
        root.read_with(cx, |root, _| assert_eq!(root.view, View::Events));
    }

    #[gpui_kit::test]
    async fn without_events_k8s_io_the_core_api_lists_events(cx: &mut TestAppContext) {
        let (handle, root) = open_events("", Vec::new(), cx).await;

        wait_for_label(handle, "pod/api-1", cx).await;
        root.read_with(cx, |root, _| assert!(root.core_events));
        press(handle, "j", cx);
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.try_find_by_label("Go to pod/api-1").is_some()
        })
        .await;
    }

    #[gpui_kit::test]
    async fn a_refused_event_list_shows_no_permission(cx: &mut TestAppContext) {
        let (handle, root) = open_events(EVENT_LIST_BODY, vec![EVENTS], cx).await;

        wait_for_notice(handle, "You can't list Events across all namespaces", cx).await;
        root.read_with(cx, |root, cx| {
            let locked: Vec<_> = root
                .sidebar_items(cx)
                .into_iter()
                .filter(|item| item.forbidden)
                .map(|item| item.view)
                .collect();
            assert_eq!(locked, [View::Events]);
        });
    }

    #[gpui_kit::test]
    async fn restores_the_saved_view_and_its_columns(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_deployments(Vec::new()), "dev"));
        init(cx);
        let saved = SavedState {
            view: Some("Deployments".into()),
            view_columns: [("Deployments".to_owned(), vec!["Age".to_owned()])].into(),
            hidden_columns: vec!["Node".into()],
            ..SavedState::default()
        };
        let backend = Arc::new(Backend::new().unwrap());
        let (handle, root) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Root::new(backend, contexts, saved, window, cx))
            })
            .unwrap()
        });
        wait_for_label(handle, "api", cx).await;

        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.find_all_by_label("Filter deployments").len(), 1);
            assert!(window.try_find(("sort", 6usize)).is_none());
            let state = root.read(cx).saved_state(window, cx);
            assert_eq!(state.view.as_deref(), Some("Deployments"));
            assert_eq!(state.view_columns["Deployments"], ["Age"]);
            assert_eq!(state.hidden_columns, ["Node"]);
        })
        .unwrap();
    }
    const OWNED_POD_BODY: &str = r#"{"kind":"Pod","apiVersion":"v1","metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1","ownerReferences":[{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"api-7f9c","uid":"r","controller":true}]},"spec":{"nodeName":"node-1","containers":[{"name":"app","image":"api:1"}]},"status":{"phase":"Running"}}"#;
    const SHOP_DEPLOYMENTS: &str = "/apis/apps/v1/namespaces/shop/deployments";

    /// The `api` Deployment, its ReplicaSet and a pod they own. `forbidden` lists paths the
    /// API refuses.
    async fn open_links(
        forbidden: Vec<&'static str>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let server = serve(Fixture {
            pod_list: OWNED_POD_LIST_BODY.into(),
            pod: OWNED_POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
            live_logs: None,
            forbidden,
            reconnect_logs: None,
            objects: vec![
                (DEPLOYMENTS.into(), DEPLOYMENT_LIST_BODY.into()),
                (SHOP_DEPLOYMENTS.into(), DEPLOYMENT_LIST_BODY.into()),
                (
                    "/apis/apps/v1/namespaces/shop/deployments/api".into(),
                    DEPLOYMENT_BODY.into(),
                ),
                (
                    "/apis/apps/v1/namespaces/shop/replicasets/api-7f9c".into(),
                    REPLICA_SET_BODY.into(),
                ),
            ],
        });
        let (handle, root) = open_root(Contexts::from_yaml(&kubeconfig(&server, "dev")), cx);
        wait_for_pods(handle, cx).await;
        press(handle, "j", cx);
        wait_for_label(handle, "Go to deployment/api", cx).await;
        (handle, root)
    }

    async fn wait_for_details(handle: AnyWindowHandle, name: &str, cx: &mut TestAppContext) {
        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window
                .try_find("details-title")
                .is_some_and(|title| title.label() == Some(name))
        })
        .await;
    }

    #[gpui_kit::test]
    async fn a_pod_links_to_its_deployment_and_back(cx: &mut TestAppContext) {
        let (handle, root) = open_links(Vec::new(), cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let link = window.find_by_label("Go to deployment/api");
            assert_eq!(link.role(), Some(gpui_kit::Role::Link));
            assert!(
                window
                    .try_find_by_label("Go to replicaset/api-7f9c")
                    .is_none()
            );
            assert!(!window.find_all_by_label("replicaset/api-7f9c").is_empty());
            window.click("link deployment/api", cx);
        })
        .unwrap();

        wait_for_label(handle, "Filter deployments", cx).await;
        wait_for_details(handle, "api", cx).await;
        root.read_with(cx, |root, _| assert_eq!(root.view, View::Deployments));

        press(handle, BACK_KEY, cx);
        wait_for_details(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find_all_by_label("Filter pods").len(), 1);
            assert!(window.try_find("go-back").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_link_follows_from_the_keyboard(cx: &mut TestAppContext) {
        let (handle, _) = open_links(Vec::new(), cx).await;

        let mut focused = false;
        for _ in 0..40 {
            press(handle, "tab", cx);
            focused = cx
                .update_window(handle, |_, window, _| {
                    window.find_by_label("Go to deployment/api").focused() == Some(true)
                })
                .unwrap();
            if focused {
                break;
            }
        }
        assert!(focused, "Tab never reached the link");
        press(handle, "enter", cx);

        wait_for_details(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("go-back").label(), Some("Back to pod api-1"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_kind_without_a_view_shows_as_text_until_it_has_one(cx: &mut TestAppContext) {
        let (handle, _) = open_links(Vec::new(), cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let node_link = window.try_find_by_label("Go to node/node-1");
            match View::for_kind("Node") {
                Some(_) => assert!(node_link.is_some()),
                None => {
                    assert!(node_link.is_none());
                    assert!(window.try_find("text node/node-1").is_some());
                }
            }
            let set_link = window.try_find_by_label("Go to replicaset/api-7f9c");
            assert_eq!(set_link.is_some(), View::for_kind("ReplicaSet").is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_link_to_a_refused_kind_shows_no_permission(cx: &mut TestAppContext) {
        let (handle, root) = open_links(vec![DEPLOYMENTS, SHOP_DEPLOYMENTS], cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("link deployment/api", cx);
        })
        .unwrap();

        wait_for_notice(handle, "You can't list Deployments in shop", cx).await;
        root.read_with(cx, |root, cx| {
            assert_eq!(root.view, View::Deployments);
            assert_eq!(root.namespace(cx).as_deref(), Some("shop"));
        });
    }

    #[gpui_kit::test]
    async fn a_link_switches_namespace_and_back_restores_it(cx: &mut TestAppContext) {
        let (handle, root) = open_links(Vec::new(), cx).await;
        set_namespace(handle, &root, "web", cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("link deployment/api", cx);
        })
        .unwrap();

        wait_for_details(handle, "api", cx).await;
        root.read_with(cx, |root, cx| {
            assert_eq!(root.namespace(cx).as_deref(), Some("shop"));
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("go-back", cx);
        })
        .unwrap();
        wait_for_details(handle, "api-1", cx).await;
        root.read_with(cx, |root, cx| {
            assert_eq!(root.view, View::Pods);
            assert_eq!(root.namespace(cx).as_deref(), Some("web"));
        });
    }

    #[gpui_kit::test]
    async fn back_returns_from_a_workloads_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_deployments(Vec::new(), cx).await;
        press(handle, "2", cx);
        wait_for_label(handle, "api", cx).await;
        press(handle, "j", cx);
        press(handle, "p", cx);
        wait_for_label(handle, "Back to deployment api", cx).await;

        press(handle, BACK_KEY, cx);
        wait_for_details(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find_all_by_label("Filter deployments").len(), 1);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn choosing_a_view_forgets_where_back_went(cx: &mut TestAppContext) {
        let (handle, _) = open_links(Vec::new(), cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("link deployment/api", cx);
        })
        .unwrap();
        wait_for_details(handle, "api", cx).await;

        press(handle, "1", cx);
        cx.update_window(handle, |_, window, _| {
            assert!(window.try_find("go-back").is_none());
        })
        .unwrap();
    }

    async fn open_workloads(
        served: String,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&served, "dev"));
        let (handle, root) = open_root(contexts, cx);
        wait_for_pods(handle, cx).await;
        (handle, root)
    }

    fn serve_stateful_sets(forbidden: Vec<&'static str>) -> String {
        serve_workload(
            "apps/v1",
            "statefulsets",
            STATEFUL_SET_LIST_BODY,
            STATEFUL_SET_BODY,
            forbidden,
        )
    }

    fn locked_views(root: &Entity<Root>, cx: &mut TestAppContext) -> Vec<View> {
        root.read_with(cx, |root, cx| {
            root.sidebar_items(cx)
                .into_iter()
                .filter(|item| item.forbidden)
                .map(|item| item.view)
                .collect()
        })
    }

    #[gpui_kit::test]
    async fn a_stateful_set_opens_its_details_and_shows_its_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_stateful_sets(Vec::new()), cx).await;
        press(handle, "3", cx);
        wait_for_label(handle, "Filter statefulsets", cx).await;
        wait_for_label(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(window.find_all_by_label("1/1").len(), 1);
        })
        .unwrap();

        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details-title").label(), Some("api"));
            assert_eq!(window.find("details").label(), Some("StatefulSet details"));
        })
        .unwrap();

        press(handle, "p", cx);
        wait_for_label(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(
                window.find("selector-filter").label(),
                Some("Showing pods of statefulset/api")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_w_key_follows_the_selected_stateful_sets_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_stateful_sets(Vec::new()), cx).await;
        press(handle, "3", cx);
        wait_for_label(handle, "api", cx).await;

        press(handle, "j", cx);
        press(handle, "w", cx);

        wait_for_log_summary(handle, "Following statefulset/api", cx).await;
    }

    #[gpui_kit::test]
    async fn a_refused_stateful_set_list_shows_no_permission(cx: &mut TestAppContext) {
        let (handle, root) = open_workloads(serve_stateful_sets(vec![STATEFUL_SETS]), cx).await;

        press(handle, "3", cx);
        wait_for_notice(
            handle,
            "You can't list StatefulSets across all namespaces",
            cx,
        )
        .await;
        assert_eq!(locked_views(&root, cx), [View::StatefulSets]);
    }

    fn serve_daemon_sets(forbidden: Vec<&'static str>) -> String {
        serve_workload(
            "apps/v1",
            "daemonsets",
            DAEMON_SET_LIST_BODY,
            DAEMON_SET_BODY,
            forbidden,
        )
    }

    #[gpui_kit::test]
    async fn a_daemon_set_opens_its_details_and_shows_its_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_daemon_sets(Vec::new()), cx).await;
        press(handle, "4", cx);
        wait_for_label(handle, "Filter daemonsets", cx).await;
        wait_for_label(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(window.find_all_by_label("Misscheduled").len(), 1);
            assert_eq!(window.find_all_by_label("kubernetes.io/os=linux").len(), 1);
        })
        .unwrap();

        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details-title").label(), Some("api"));
            assert_eq!(window.find("details").label(), Some("DaemonSet details"));
        })
        .unwrap();

        press(handle, "p", cx);
        wait_for_label(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(
                window.find("selector-filter").label(),
                Some("Showing pods of daemonset/api")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_l_key_follows_the_selected_daemon_sets_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_daemon_sets(Vec::new()), cx).await;
        press(handle, "4", cx);
        wait_for_label(handle, "api", cx).await;

        press(handle, "j", cx);
        press(handle, "l", cx);

        wait_for_log_summary(handle, "Following daemonset/api", cx).await;
    }

    #[gpui_kit::test]
    async fn a_refused_daemon_set_list_shows_no_permission(cx: &mut TestAppContext) {
        let (handle, root) = open_workloads(serve_daemon_sets(vec![DAEMON_SETS]), cx).await;

        press(handle, "4", cx);
        wait_for_notice(
            handle,
            "You can't list DaemonSets across all namespaces",
            cx,
        )
        .await;
        assert_eq!(locked_views(&root, cx), [View::DaemonSets]);
    }

    fn serve_jobs(forbidden: Vec<&'static str>) -> String {
        serve_workload("batch/v1", "jobs", JOB_LIST_BODY, JOB_BODY, forbidden)
    }

    #[gpui_kit::test]
    async fn a_job_opens_its_details_and_shows_its_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_jobs(Vec::new()), cx).await;
        press(handle, "5", cx);
        wait_for_label(handle, "Filter jobs", cx).await;
        wait_for_label(handle, "api", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(window.find_all_by_label("Running").len(), 1);
            assert_eq!(window.find_all_by_label("1/2").len(), 1);
            assert_eq!(window.find_all_by_label("nightly").len(), 1);
        })
        .unwrap();

        press(handle, "j", cx);
        wait_for_label(handle, "cronjob/nightly", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("details-title").label(), Some("api"));
            assert_eq!(window.find("details").label(), Some("Job details"));
            assert!(window.try_find("link cronjob/nightly").is_none());
        })
        .unwrap();

        press(handle, "p", cx);
        wait_for_label(handle, "api-1", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert!(window.find_all_by_label("web-1").is_empty());
            assert_eq!(
                window.find("selector-filter").label(),
                Some("Showing pods of job/api")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_w_key_follows_the_selected_jobs_pods(cx: &mut TestAppContext) {
        let (handle, _) = open_workloads(serve_jobs(Vec::new()), cx).await;
        press(handle, "5", cx);
        wait_for_label(handle, "api", cx).await;

        press(handle, "j", cx);
        press(handle, "w", cx);

        wait_for_log_summary(handle, "Following job/api", cx).await;
    }

    #[gpui_kit::test]
    async fn a_refused_job_list_shows_no_permission(cx: &mut TestAppContext) {
        let (handle, root) = open_workloads(serve_jobs(vec![JOBS]), cx).await;

        press(handle, "5", cx);
        wait_for_notice(handle, "You can't list Jobs across all namespaces", cx).await;
        assert_eq!(locked_views(&root, cx), [View::Jobs]);
    }

    /// The rows of the table, as a screen reader finds them, without the header row.
    fn table_rows(window: &gpui_kit::Window) -> Vec<gpui_kit::test::ElementSnapshot> {
        window
            .find_all_by_role(gpui_kit::Role::Row)
            .into_iter()
            .filter(|row| row.selected().is_some())
            .collect()
    }

    fn selected_row_label(window: &gpui_kit::Window) -> Option<String> {
        table_rows(window)
            .into_iter()
            .find(|row| row.selected() == Some(true))
            .and_then(|row| row.label().map(str::to_owned))
    }

    #[gpui_kit::test]
    async fn the_selected_row_reads_as_selected_and_takes_focus(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(selected_row_label(window), None);
            assert_eq!(window.find("table").role(), Some(gpui_kit::Role::Table));
            assert_eq!(window.find("table").label(), Some("Pods"));
        })
        .unwrap();
        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            let label = selected_row_label(window).unwrap();
            assert!(
                label.starts_with("api-1, Namespace shop, Status Running"),
                "{label}"
            );
            assert_eq!(
                table_rows(window)
                    .iter()
                    .filter(|row| row.selected() == Some(true))
                    .count(),
                1
            );
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();

        press(handle, "y", cx);
        cx.update_window(handle, |_, window, _| {
            let tabs: Vec<_> = window
                .find_all_by_role(gpui_kit::Role::Tab)
                .iter()
                .map(|tab| (tab.label().unwrap_or_default().to_owned(), tab.selected()))
                .collect();
            assert_eq!(tabs.len(), 3, "{tabs:?}");
            assert_eq!(tabs[0], ("Summary".to_owned(), Some(false)));
            assert_eq!(tabs[2], ("YAML".to_owned(), Some(true)));
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();

        press(handle, "down", cx);
        cx.update_window(handle, |_, window, _| {
            assert!(selected_row_label(window).unwrap().starts_with("front-1"));
            assert_eq!(window.find("details-title").label(), Some("front-1"));
        })
        .unwrap();
        press(handle, "home", cx);
        cx.update_window(handle, |_, window, _| {
            assert!(selected_row_label(window).unwrap().starts_with("api-1"));
        })
        .unwrap();
        press(handle, "end", cx);
        cx.update_window(handle, |_, window, _| {
            assert!(selected_row_label(window).unwrap().starts_with("front-1"));
        })
        .unwrap();

        // Left and right would select a column of GPUI Kit's table.
        press(handle, "left", cx);
        press(handle, "escape", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(selected_row_label(window), None);
            assert!(window.try_find("details").is_none());
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn a_clicked_row_reads_as_selected(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 1usize), cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(selected_row_label(window).unwrap().starts_with("front-1"));
            assert_eq!(window.find("details-title").label(), Some("front-1"));
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_selection_stays_marked_as_rows_move(cx: &mut TestAppContext) {
        let handle = connected(cx).await;
        press(handle, "end", cx);
        cx.update_window(handle, |_, window, cx| {
            assert!(selected_row_label(window).unwrap().starts_with("front-1"));
            window.dispatch_action(Box::new(super::SortBy { column: "Name" }), cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_action(Box::new(super::SortBy { column: "Name" }), cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let rows = table_rows(window);
            assert!(rows[0].label().unwrap().starts_with("front-1"));
            assert_eq!(rows[0].selected(), Some(true));
            assert_eq!(rows[1].selected(), Some(false));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_palette_sorts_by_each_column(cx: &mut TestAppContext) {
        let handle = connected(cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            assert_eq!(window.find("palette").role(), Some(gpui_kit::Role::Dialog));
            window.input("sort by status", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(palette_options(window), ["Sort by Status"]);
            window.press("enter", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            window.input("sort", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let options = palette_options(window);
            assert!(options.contains(&"Sort by Status, descending".to_owned()));
            assert!(options.contains(&"Sort by Name".to_owned()));
        })
        .unwrap();
    }

    fn palette_options(window: &gpui_kit::Window) -> Vec<String> {
        window
            .find_all_by_role(gpui_kit::Role::ListBoxOption)
            .iter()
            .filter_map(|option| option.label().map(str::to_owned))
            .collect()
    }

    #[gpui_kit::test]
    async fn tab_leaves_the_table_and_closing_the_palette_returns_to_it(cx: &mut TestAppContext) {
        let handle = connected(cx).await;
        press(handle, "j", cx);

        cx.update_window(handle, |_, window, cx| {
            window.press(OPEN_PALETTE_KEY, cx);
            window.render_frame(cx);
            assert_ne!(window.find("table").focused(), Some(true));
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_none());
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();

        press(handle, "?", cx);
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                window.find(("shortcut", 0usize)).label(),
                Some(
                    format!(
                        "{}: Open the command palette",
                        super::super::palette::SHORTCUTS[0].0
                    )
                    .as_str()
                )
            );
            window.press("escape", cx);
            window.render_frame(cx);
            assert!(window.try_find("overlay").is_none());
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();

        press(handle, "tab", cx);
        cx.update_window(handle, |_, window, _| {
            assert_ne!(window.find("table").focused(), Some(true));
        })
        .unwrap();
        press(handle, "shift-tab", cx);
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn closing_the_details_from_a_link_returns_focus_to_the_table(cx: &mut TestAppContext) {
        let (handle, _) = open_links(Vec::new(), cx).await;

        let mut focused = false;
        for _ in 0..40 {
            press(handle, "tab", cx);
            focused = cx
                .update_window(handle, |_, window, _| {
                    window.find_by_label("Go to deployment/api").focused() == Some(true)
                })
                .unwrap();
            if focused {
                break;
            }
        }
        assert!(focused, "Tab never reached the link");
        press(handle, "escape", cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("details").is_none());
            assert_eq!(window.find("table").focused(), Some(true));
        })
        .unwrap();
        press(handle, "j", cx);
        cx.update_window(handle, |_, window, _| {
            assert!(selected_row_label(window).is_some());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn the_filter_takes_composed_text(cx: &mut TestAppContext) {
        let (handle, root) = open_root(
            Contexts::from_yaml(&kubeconfig(&serve_pods(TWO_NAMESPACES_BODY), "dev")),
            cx,
        );
        cx.executor().allow_parking();
        wait_for_pods(handle, cx).await;
        press(handle, "/", cx);
        let filter = root.read_with(cx, |root, _| root.filter.clone());

        cx.update_window(handle, |_, window, cx| {
            let bounds = window.find("pod-filter").bounds();
            let handler = gpui_kit::ElementInputHandler::new(bounds, filter.clone());
            let mut input = gpui_kit::test::TestInput::new(handler, window);
            // An IME marks text while it composes, then commits it.
            input.compose_text(None, "ｆｒ", Some(0..2), cx);
            assert_eq!(input.marked_text_range(cx), Some(0..2));
            input.compose_text(None, "fr", Some(0..2), cx);
            input.unmark_text(cx);
            assert_eq!(input.marked_text_range(cx), None);
            input.commit_text("ont", cx);
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find_all_by_label("api-1").is_empty());
            assert_eq!(window.find_all_by_label("front-1").len(), 1);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn log_lines_are_options_of_a_list(cx: &mut TestAppContext) {
        let handle = connected(cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tail-logs", cx);
        })
        .unwrap();
        wait_for_label(handle, "api-1/app 2026-01-01T00:00:00Z first line", cx).await;
        cx.update_window(handle, |_, window, _| {
            assert_eq!(
                window.find("log-lines").role(),
                Some(gpui_kit::Role::ListBox)
            );
            let line = window.find_by_label("api-1/app 2026-01-01T00:00:00Z first line");
            assert_eq!(line.role(), Some(gpui_kit::Role::ListBoxOption));
            assert_eq!(line.selected(), Some(false));
            assert_eq!(
                window.find("log-timeline").role(),
                Some(gpui_kit::Role::ListBox)
            );
            assert_eq!(
                window.find("log-summary").role(),
                Some(gpui_kit::Role::Status)
            );
            assert_eq!(
                window.find_by_label("Export lines").role(),
                Some(gpui_kit::Role::Button)
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn labelled_text_reaches_the_accessibility_tree(cx: &mut TestAppContext) {
        let handle = connected(cx).await;
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // A label without a role never reaches the platform's tree.
            for element in window.elements() {
                if element.label().is_some() {
                    assert!(element.role().is_some(), "{:?} has no role", element.path());
                }
            }
            assert_eq!(
                window.find("resource-tree").role(),
                Some(gpui_kit::Role::Tree)
            );
        })
        .unwrap();
    }
}
