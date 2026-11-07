use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{Disableable, IndexPath, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Backend, ClusterError, Connection, Contexts, LogTarget, PodSnapshot};

use crate::logs::LogPane;
use crate::pods::PodTable;

type ContextPicker = SelectState<Vec<String>>;

pub struct Root {
    backend: Arc<Backend>,
    contexts: Option<Contexts>,
    picker: Option<Entity<ContextPicker>>,
    pods: Entity<TableState<PodTable>>,
    filter: Entity<InputState>,
    connection: Option<Connection>,
    logs: Option<Entity<LogPane>>,
    status: SharedString,
    session: Option<Task<()>>,
    _picker_events: Option<Subscription>,
    _filter_events: Subscription,
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
                .sortable(false)
                .row_selectable(false)
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter pods"));
        let filter_events = cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let filter = filter.read(cx).value().to_string();
                this.pods.update(cx, |pods, cx| {
                    pods.delegate_mut().set_filter(filter);
                    cx.notify();
                });
                cx.notify();
            }
        });
        let mut root = Self {
            backend,
            contexts: None,
            picker: None,
            pods,
            filter,
            connection: None,
            logs: None,
            status: "Choose a context".into(),
            session: None,
            _picker_events: None,
            _filter_events: filter_events,
        };
        match contexts {
            Ok(contexts) => root.show_contexts(contexts, window, cx),
            Err(error) => root.status = error.to_string().into(),
        }
        root
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
            |this, _, event: &SelectEvent<Vec<String>>, _, cx| {
                let SelectEvent::Confirm(Some(context)) = event else {
                    return;
                };
                this.connect(context.clone(), cx);
            },
        ));
        self.picker = Some(picker);
        self.contexts = Some(contexts);
        if let Some(current) = current {
            self.connect(current, cx);
        }
    }

    fn connect(&mut self, context: String, cx: &mut Context<Self>) {
        let Some(contexts) = self.contexts.clone() else {
            return;
        };
        self.status = format!("Connecting to {context}").into();
        self.connection = None;
        self.logs = None;
        self.show_pods(Arc::default(), cx);
        let connecting = self.backend.connect(contexts, context);
        let backend = self.backend.clone();
        // Replacing the task drops any earlier session and its watch, so a slow
        // context cannot overwrite a newer choice.
        self.session = Some(cx.spawn(async move |this, cx| {
            let connection = match connecting.await {
                Ok(connection) => connection,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.status = error.to_string().into();
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let mut pods = backend.watch_pods(&connection);
            let summary = connection_summary(&connection);
            this.update(cx, |this, cx| {
                this.status = format!("{summary}. Loading pods").into();
                this.connection = Some(connection);
                cx.notify();
            })
            .ok();
            while let Some(snapshot) = pods.next().await {
                let updated = this.update(cx, |this, cx| {
                    this.status = pod_status(&summary, &snapshot).into();
                    this.show_pods(snapshot, cx);
                });
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
            Err(error) => self.status = format!("Failed to start log tail: {error}").into(),
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

    fn log_targets(&self, cx: &App) -> Vec<LogTarget> {
        let table = self.pods.read(cx).delegate();
        table.snapshot().log_targets(table.filter())
    }

    fn show_pods(&mut self, snapshot: Arc<PodSnapshot>, cx: &mut Context<Self>) {
        self.pods.update(cx, |pods, cx| {
            pods.delegate_mut().show(snapshot);
            cx.notify();
        });
        cx.notify();
    }
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

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let streams = self.log_targets(cx).len();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .when_some(self.picker.as_ref(), |toolbar, picker| {
                        toolbar.child(
                            div().w(px(320.)).child(
                                Select::new(picker)
                                    .id("context-picker")
                                    .accessibility_label("Kubernetes context")
                                    .placeholder("Choose a context"),
                            ),
                        )
                    })
                    .child(
                        div().w(px(320.)).child(
                            Input::new(&self.filter)
                                .id("pod-filter")
                                .aria_label("Filter pods"),
                        ),
                    )
                    .child(
                        Button::new("tail-logs")
                            .small()
                            .outline()
                            .label(format!("Tail logs ({streams})"))
                            .disabled(streams == 0 || self.connection.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.tail_logs(cx))),
                    ),
            )
            .child(
                div()
                    .id("status")
                    .role(Role::Status)
                    .test_support()
                    .aria_label(self.status.clone())
                    .child(self.status.clone()),
            )
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.pods)
                        .accessibility_label("Pods")
                        .stripe(true),
                ),
            )
            .when_some(self.logs.clone(), |root, logs| {
                root.child(div().flex_1().min_h_0().child(logs))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::{Root, pod_status};

    use gpui_kit::test::{TestAppContextExt, TestQueryExt, TestWindowExt};
    use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, WindowOptions};
    use taffrail_core::{Backend, Contexts, PodSnapshot};

    const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;

    const POD_LIST_BODY: &str = r#"{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"1"},"items":[{"metadata":{"namespace":"shop","name":"api-1","resourceVersion":"1"},"spec":{"nodeName":"node-1","containers":[{"name":"app"}]},"status":{"phase":"Running"}}]}"#;
    const LOG_BODY: &str = "2026-01-01T00:00:00Z first line\n";

    fn kubeconfig(server: &str, current: &str) -> String {
        format!(
            r#"
current-context: {current}
clusters:
  - name: dev
    cluster:
      server: {server}
contexts:
  - name: dev
    context:
      cluster: dev
      user: dev
users:
  - name: dev
    user: {{}}
"#
        )
    }

    fn respond(mut stream: &TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn serve_request(stream: TcpStream) {
        // Closing with unread data resets the connection on Windows, which loses the response.
        let mut request = BufReader::new(&stream);
        let mut request_line = String::new();
        request.read_line(&mut request_line).unwrap();
        let mut header = String::new();
        while request.read_line(&mut header).unwrap() > "\r\n".len() {
            header.clear();
        }
        if request_line.contains("watch=true") {
            thread::park();
        } else if request_line.contains("/api/v1/pods") {
            respond(&stream, POD_LIST_BODY);
        } else if request_line.contains("/log?") {
            respond(&stream, LOG_BODY);
        } else {
            respond(&stream, VERSION_BODY);
        }
    }

    fn serve_api() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                thread::spawn(move || serve_request(stream));
            }
        });
        server
    }

    fn open(
        contexts: Result<Contexts, taffrail_core::ClusterError>,
        cx: &mut TestAppContext,
    ) -> AnyWindowHandle {
        cx.update(gpui_kit::init);
        let backend = Arc::new(Backend::new().unwrap());
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Root::new(backend, contexts, window, cx))
            })
            .unwrap()
            .0
        })
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
    async fn filtering_narrows_the_table_and_the_tail(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_api(), "dev"));
        let handle = open(contexts, cx);
        wait_for_pods(handle, cx).await;

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_label("api-1").len(), 1);
            assert_eq!(window.find("tail-logs").label(), Some("Tail logs (1)"));
            window.click("pod-filter", cx);
            window.input("web", cx);
        })
        .unwrap();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find_all_by_label("api-1").is_empty());
            assert_eq!(window.find("tail-logs").label(), Some("Tail logs (0)"));
        })
        .unwrap();
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
