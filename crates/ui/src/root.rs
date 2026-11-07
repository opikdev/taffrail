use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, Size, StyledExt, Theme, TitleBar, h_flex,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{
    Backend, ClusterError, Connection, Contexts, LogTarget, MAX_TAIL_PODS, PodSnapshot,
};

use crate::details::{DETAILS_WIDTH, DetailsEvent, DetailsPane};
use crate::logs::{LogPane, dock_height};
use crate::pods::{ALL_NAMESPACES, PodTable, show_in_place};
use crate::sidebar::resource_sidebar;

type ContextPicker = SelectState<Vec<String>>;
type NamespacePicker = SelectState<SearchableVec<String>>;

actions!(taffrail, [NextRow, PreviousRow, CloseDetails]);

const KEY_CONTEXT: &str = "Taffrail";
const TABLE_KEYS: &str = "Taffrail && !Input";

/// Binds the keys that act on the pod table when no text field has focus.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", NextRow, Some(TABLE_KEYS)),
        KeyBinding::new("down", NextRow, Some(TABLE_KEYS)),
        KeyBinding::new("k", PreviousRow, Some(TABLE_KEYS)),
        KeyBinding::new("up", PreviousRow, Some(TABLE_KEYS)),
        KeyBinding::new("escape", CloseDetails, Some(TABLE_KEYS)),
    ]);
}

const TITLE_BAR_HEIGHT: f32 = 40.;
const PANEL_HEADER_HEIGHT: f32 = 44.;
const CONTEXT_PICKER_CHAR_WIDTH: f32 = 7.5;
const CONTEXT_PICKER_CHROME: f32 = 40.;
const CONTEXT_PICKER_WIDTH: Range<f32> = 96.0..260.0;
const CONTEXT_BADGE_SIZE: f32 = 20.;
const FILTER_WIDTH: f32 = 240.;
const NAMESPACE_PICKER_WIDTH: f32 = 148.;
const NAMESPACE_MENU_WIDTH: f32 = 240.;
const NOTICE_WIDTH: f32 = 420.;
const CONNECTION_DOT: f32 = 7.;

/// Window chrome with the traffic lights centred in the title bar.
pub fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(point(px(14.), px(13.))),
    }
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
enum NoticeAction {
    ReloadContexts,
    RetryConnection,
    ClearFilter,
    ShowAllNamespaces,
}

struct Notice {
    title: SharedString,
    detail: Option<SharedString>,
    action: Option<(NoticeAction, &'static str)>,
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

    fn action(mut self, action: NoticeAction, label: &'static str) -> Self {
        self.action = Some((action, label));
        self
    }
}

pub struct Root {
    backend: Arc<Backend>,
    contexts: Option<Contexts>,
    picker: Option<Entity<ContextPicker>>,
    pods: Entity<TableState<PodTable>>,
    filter: Entity<InputState>,
    namespace_picker: Entity<NamespacePicker>,
    namespace_options: Vec<String>,
    connection: Option<Connection>,
    logs: Option<Entity<LogPane>>,
    details: Option<Entity<DetailsPane>>,
    focus_handle: FocusHandle,
    session: Session,
    sidebar_collapsed: bool,
    connect_task: Option<Task<()>>,
    _picker_events: Option<Subscription>,
    _filter_events: Subscription,
    _namespace_events: Subscription,
    _table_events: Subscription,
    _details_events: Option<Subscription>,
    _appearance: Subscription,
}

impl Root {
    pub fn new(
        backend: Arc<Backend>,
        contexts: Result<Contexts, ClusterError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pods = cx.new(|cx| {
            TableState::new(PodTable::default(), window, cx)
                .col_movable(false)
                .col_resizable(false)
                .row_selectable(false)
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter pods"));
        let filter_events = cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let filter = filter.read(cx).value().to_string();
                this.apply_filter(filter, cx);
            }
        });
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
            },
        );
        let table_events =
            cx.subscribe_in(&pods, window, |this, _, event: &TableEvent, window, cx| {
                if matches!(event, TableEvent::SelectRow(_)) {
                    this.focus_handle.focus(window, cx);
                    this.open_details(cx);
                }
            });
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        let mut root = Self {
            backend,
            contexts: None,
            picker: None,
            pods,
            filter,
            namespace_picker,
            namespace_options,
            connection: None,
            logs: None,
            details: None,
            focus_handle,
            session: Session::ChooseContext,
            sidebar_collapsed: false,
            connect_task: None,
            _picker_events: None,
            _filter_events: filter_events,
            _namespace_events: namespace_events,
            _table_events: table_events,
            _details_events: None,
            _appearance: appearance,
        };
        root.load_contexts(contexts, window, cx);
        root
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
        let current = contexts.current().map(str::to_owned);
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
            },
        ));
        self.picker = Some(picker);
        self.contexts = Some(contexts);
        self.session = Session::ChooseContext;
        if let Some(current) = current {
            self.connect(current, window, cx);
        }
    }

    fn connect(&mut self, context: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(contexts) = self.contexts.clone() else {
            return;
        };
        self.session = Session::Connecting {
            context: context.clone(),
        };
        self.connection = None;
        self.logs = None;
        self.close_details(cx);
        self.show_pods(Arc::default(), window, cx);
        let connecting = self.backend.connect(contexts, context.clone());
        let backend = self.backend.clone();
        // Replacing the task drops any earlier session and its watch, so a slow
        // context cannot overwrite a newer choice.
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
            let mut pods = backend.watch_pods(&connection);
            this.update(cx, |this, cx| {
                this.session = Session::Connected {
                    summary: connection_summary(&connection),
                    version: connection.server_version.clone(),
                };
                this.connection = Some(connection);
                cx.notify();
            })
            .ok();
            while let Some(snapshot) = pods.next().await {
                let updated =
                    this.update_in(cx, |this, window, cx| this.show_pods(snapshot, window, cx));
                if updated.is_err() {
                    return;
                }
            }
        }));
        cx.notify();
    }

    pub(crate) fn tail_logs(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        match self.backend.tail_logs(connection, self.log_targets(cx)) {
            Ok(tail) => self.logs = Some(cx.new(|cx| LogPane::new(tail, cx))),
            Err(error) => {
                self.session = Session::Failed {
                    context: connection.context.clone(),
                    error: format!("Failed to start log tail: {error}").into(),
                }
            }
        }
        cx.notify();
    }

    #[cfg(feature = "bench")]
    pub(crate) fn synced_pod_count(&self, cx: &App) -> Option<usize> {
        let snapshot = self.pods.read(cx).delegate().snapshot();
        (self.connection.is_some() && snapshot.synced).then_some(snapshot.rows.len())
    }

    #[cfg(feature = "bench")]
    pub(crate) fn scroll_pods_to(&mut self, row: usize, cx: &mut Context<Self>) {
        self.pods.update(cx, |pods, cx| pods.scroll_to_row(row, cx));
    }

    #[cfg(feature = "preview")]
    pub(crate) fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    fn open_details(&mut self, cx: &mut Context<Self>) {
        let table = self.pods.read(cx).delegate();
        let selected = table.selected().and_then(|key| table.snapshot().find(key));
        let (Some(row), Some(connection)) = (selected.cloned(), self.connection.clone()) else {
            return;
        };
        let backend = self.backend.clone();
        let details = cx.new(|cx| DetailsPane::new(backend, connection, row, cx));
        self._details_events = Some(cx.subscribe(&details, |this, _, event, cx| match event {
            DetailsEvent::Close => this.close_details(cx),
        }));
        self.details = Some(details);
        cx.notify();
    }

    fn close_details(&mut self, cx: &mut Context<Self>) {
        self.details = None;
        self._details_events = None;
        self.pods.update(cx, |pods, cx| {
            pods.delegate_mut().clear_selection();
            cx.notify();
        });
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let moved = self.pods.update(cx, |pods, cx| {
            let moved = pods.delegate_mut().move_selection(delta);
            if let Some(row_ix) = moved {
                pods.vertical_scroll_handle
                    .scroll_to_item(row_ix, ScrollStrategy::Nearest);
            }
            cx.notify();
            moved
        });
        if moved.is_some() {
            self.open_details(cx);
        }
    }

    /// Selects a row as a click would.
    #[cfg(feature = "preview")]
    pub(crate) fn select_first_row(&mut self, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn log_targets(&self, cx: &App) -> Vec<LogTarget> {
        self.pods.read(cx).delegate().log_targets()
    }

    fn show_pods(
        &mut self,
        snapshot: Arc<PodSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pods
            .update(cx, |pods, cx| show_in_place(pods, snapshot, cx));
        self.update_namespace_options(window, cx);
        self.update_details(cx);
        cx.notify();
    }

    /// Snapshots arrive up to 10 times a second, and most leave the namespaces alone.
    fn update_namespace_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.pods.read(cx).delegate().namespace_options();
        if options == self.namespace_options {
            return;
        }
        let options: Vec<String> = options.into_iter().map(str::to_owned).collect();
        self.namespace_options.clone_from(&options);
        self.namespace_picker.update(cx, |picker, cx| {
            picker.set_items(options.into(), window, cx);
        });
    }

    pub(crate) fn namespace(&self, cx: &App) -> Option<String> {
        self.pods.read(cx).delegate().namespace().map(str::to_owned)
    }

    pub(crate) fn set_namespace(
        &mut self,
        namespace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pods.update(cx, |pods, cx| {
            pods.delegate_mut().set_namespace(namespace);
            cx.notify();
        });
        self.update_namespace_options(window, cx);
        let choice = self
            .namespace(cx)
            .unwrap_or_else(|| ALL_NAMESPACES.to_owned());
        self.namespace_picker.update(cx, |picker, cx| {
            picker.set_selected_value(&choice, window, cx);
        });
        cx.notify();
    }

    fn update_details(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.details.clone() else {
            return;
        };
        let table = self.pods.read(cx).delegate();
        let snapshot = table.snapshot();
        if !snapshot.synced {
            return;
        }
        let row = table.selected().and_then(|key| snapshot.find(key)).cloned();
        details.update(cx, |details, cx| match row {
            Some(row) => details.update_row(row, cx),
            None => details.mark_gone(cx),
        });
    }

    fn apply_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        self.pods.update(cx, |pods, cx| {
            pods.delegate_mut().set_filter(filter);
            cx.notify();
        });
        cx.notify();
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
            NoticeAction::ClearFilter => {
                self.filter
                    .update(cx, |filter, cx| filter.set_value("", window, cx));
                self.apply_filter(String::new(), cx);
            }
            NoticeAction::ShowAllNamespaces => self.set_namespace(None, window, cx),
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
                pod_status(summary, self.pods.read(cx).delegate().snapshot())
            }
        }
    }

    fn notice(&self, cx: &App) -> Option<Notice> {
        let table = self.pods.read(cx).delegate();
        let snapshot = table.snapshot();
        match &self.session {
            Session::NoClusters { error } => {
                let detail = error.clone().unwrap_or_else(|| {
                    "Taffrail reads ~/.kube/config and KUBECONFIG. Add a cluster with your provider's CLI, then reload.".into()
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
            Session::Connected { .. } if !snapshot.synced => {
                Some(snapshot.error.clone().map_or_else(
                    || Notice::new("Loading pods"),
                    |error| Notice::new("Can't list pods").detail(error),
                ))
            }
            Session::Connected { .. } if table.visible_rows() > 0 => None,
            Session::Connected { .. } => Some(no_rows_notice(table)),
        }
    }

    fn connection_state(&self, cx: &App) -> (Hsla, &'static str, Option<SharedString>) {
        let theme = cx.theme();
        match &self.session {
            Session::Connected { version, .. } => {
                let failing = self.pods.read(cx).delegate().snapshot().error.is_some();
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
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_collapsed = !this.sidebar_collapsed;
                                cx.notify();
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

    fn render_panel_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let table = self.pods.read(cx).delegate();
        let targets = self.log_targets(cx);
        let pods = distinct_pods(&targets);
        let label = tail_label(pods, table.visible_rows());
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
            .child(div().text_color(muted).child("Workloads"))
            .child(div().text_color(muted).child("/"))
            .child(div().font_medium().child("Pods"))
            .child(div().flex_1())
            .child(
                div().w(px(NAMESPACE_PICKER_WIDTH)).child(
                    Select::new(&self.namespace_picker)
                        .id("namespace-picker")
                        .small()
                        .menu_width(px(NAMESPACE_MENU_WIDTH))
                        .accessibility_label("Namespace")
                        .search_placeholder("Search namespaces"),
                ),
            )
            .child(
                div().w(px(FILTER_WIDTH)).child(
                    Input::new(&self.filter)
                        .id("pod-filter")
                        .small()
                        .prefix(Icon::new(IconName::Search).small().text_color(muted))
                        .aria_label("Filter pods"),
                ),
            )
            .child(
                Button::new("tail-logs")
                    .small()
                    .outline()
                    .icon(Icon::new(IconName::ScrollText))
                    .label(label)
                    .disabled(pods == 0 || self.connection.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.tail_logs(cx))),
            )
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
            .child(div().font_medium().child(notice.title))
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

fn no_rows_notice(table: &PodTable) -> Notice {
    if let Some(namespace) = table.empty_namespace() {
        return Notice::new(format!("No pods in {namespace}"))
            .action(NoticeAction::ShowAllNamespaces, "Show all namespaces");
    }
    if table.filter().is_empty() {
        return Notice::new("No pods in this cluster");
    }
    Notice::new(format!("No pods match \"{}\"", table.filter()))
        .action(NoticeAction::ClearFilter, "Clear filter")
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

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.pods.read(cx).delegate();
        let pod_count = table.snapshot().synced.then(|| table.snapshot().rows.len());
        let connection = self.render_connection(cx);
        let title_bar = self.render_title_bar(cx);
        let header = self.render_panel_header(cx);
        let content = match self.notice(cx) {
            Some(notice) => self.render_notice(notice, cx).into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .child(
                    DataTable::new(&self.pods)
                        .accessibility_label("Pods")
                        .with_size(Size::Small)
                        .bordered(false)
                        .stripe(false),
                )
                .into_any_element(),
        };
        let dock = dock_height(window.viewport_size().height);
        let border = cx.theme().border;
        v_flex()
            .size_full()
            .id("root")
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &NextRow, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousRow, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &CloseDetails, _, cx| this.close_details(cx)))
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(resource_sidebar(
                        pod_count,
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
                                        .when_some(self.details.clone(), |row, details| {
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

    use super::{Root, distinct_pods, no_rows_notice, pod_status, tail_label};
    use crate::fake_api::{Fixture, kubeconfig, serve};
    use crate::pods::PodTable;

    use gpui_kit::test::{TestAppContextExt, TestQueryExt, TestWindowExt};
    use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, WindowOptions};
    use taffrail_core::{Backend, Contexts, LogTarget, PodRow, PodSnapshot};

    const POD_LIST_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const TWO_NAMESPACES_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}},{"metadata":{"namespace":"web","name":"front-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const POD_BODY: &str = r#"{"kind":"Pod","apiVersion":"v1","metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app","image":"api:1"}]},"status":{"phase":"Running"}}"#;
    const EVENTS_BODY: &str = r#"{"kind":"EventList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"name":"e1","namespace":"shop"},"involvedObject":{"kind":"Pod","name":"api-1"},"type":"Warning","reason":"BackOff","message":"Back-off restarting failed container"}]}"#;
    const LOG_BODY: &str = "2026-01-01T00:00:00Z first line\n";

    fn serve_pods(pod_list: &str) -> String {
        serve(Fixture {
            pod_list: pod_list.into(),
            pod: POD_BODY.into(),
            events: EVENTS_BODY.into(),
            logs: LOG_BODY.into(),
        })
    }

    fn serve_api() -> String {
        serve_pods(POD_LIST_BODY)
    }

    fn open_root(
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<Root>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::init(cx);
        });
        let backend = Arc::new(Backend::new().unwrap());
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Root::new(backend, contexts, window, cx))
            })
            .unwrap()
        })
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

    fn pod_in(namespace: &str) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: namespace.into(),
            name: "api-1".into(),
            status: "Running".into(),
            ready: "1/1".into(),
            restarts: 0,
            node: "node-1".into(),
            created_unix: None,
            containers: vec!["app".into()],
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

    fn notice_for(table: &PodTable) -> (String, Option<&'static str>) {
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
            ("No pods in payments".into(), Some("Show all namespaces"))
        );
    }

    #[test]
    fn a_text_filter_with_no_matches_in_a_namespace_offers_to_clear_it() {
        let mut table = table_of(&["shop"]);
        table.set_namespace(Some("shop".into()));
        table.set_filter("web".into());
        assert_eq!(
            notice_for(&table),
            ("No pods match \"web\"".into(), Some("Clear filter"))
        );
    }

    #[test]
    fn an_empty_namespace_explains_itself_before_a_text_filter() {
        let mut table = table_of(&["shop"]);
        table.set_namespace(Some("payments".into()));
        table.set_filter("web".into());
        assert_eq!(
            notice_for(&table),
            ("No pods in payments".into(), Some("Show all namespaces"))
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

        snapshot.error = Some("forbidden".into());
        assert_eq!(
            pod_status("Up", &snapshot),
            "Up. Pod watch failed: forbidden"
        );
    }
}
