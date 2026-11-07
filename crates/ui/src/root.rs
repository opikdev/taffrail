use std::sync::Arc;

use gpui_kit::component::IndexPath;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{Backend, ClusterError, Contexts};

type ContextPicker = SelectState<Vec<String>>;

pub struct Root {
    backend: Arc<Backend>,
    contexts: Option<Contexts>,
    picker: Option<Entity<ContextPicker>>,
    status: SharedString,
    connecting: Option<Task<()>>,
    _picker_events: Option<Subscription>,
}

impl Root {
    pub fn new(
        backend: Arc<Backend>,
        contexts: Result<Contexts, ClusterError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut root = Self {
            backend,
            contexts: None,
            picker: None,
            status: "Choose a context".into(),
            connecting: None,
            _picker_events: None,
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
        let connection = self.backend.connect(contexts, context);
        // Replacing the task drops any earlier attempt, so a slow context cannot overwrite a newer choice.
        self.connecting = Some(cx.spawn(async move |this, cx| {
            let status = match connection.await {
                Ok(connection) => format!(
                    "Connected to {}, server {}",
                    connection.context, connection.server_version
                ),
                Err(error) => error.to_string(),
            };
            this.update(cx, |this, cx| {
                this.status = status.into();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .when_some(self.picker.as_ref(), |root, picker| {
                root.child(
                    div().w(px(320.)).child(
                        Select::new(picker)
                            .id("context-picker")
                            .accessibility_label("Kubernetes context")
                            .placeholder("Choose a context"),
                    ),
                )
            })
            .child(
                div()
                    .id("status")
                    .role(Role::Status)
                    .test_support()
                    .aria_label(self.status.clone())
                    .child(self.status.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use gpui_kit::test::{TestAppContextExt, TestWindowExt};
    use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, WindowOptions};
    use taffrail_core::{Backend, Contexts};

    use super::Root;

    const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;

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

    fn serve_version_once() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.read_exact(&mut [0; 1]).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{VERSION_BODY}",
                VERSION_BODY.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
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
    async fn connects_to_the_current_context_on_open(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let contexts = Contexts::from_yaml(&kubeconfig(&serve_version_once(), "dev"));
        let handle = open(contexts, cx);

        cx.wait_for(handle, Duration::from_secs(10), |window, _| {
            window.find("status").label() == Some("Connected to dev, server v1.35.0")
        })
        .await;
    }
}
