use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;
use taffrail_core::{Backend, ClusterError, Contexts, MAX_TAIL_PODS};

use crate::fake_api::{Fixture, kubeconfig, serve};
use crate::root::Root;

const WINDOW: (f32, f32) = (1360., 860.);
const WAIT_STEP: Duration = Duration::from_millis(20);
const WAIT_STEPS: usize = 500;
const SETTLE_FRAMES: usize = 30;

const WORKLOADS: [(&str, &str, &str, &str, u32, &str); 10] = [
    (
        "checkout-api-7f9c6d",
        "payments",
        "Waiting",
        "CrashLoopBackOff",
        7,
        "node-07",
    ),
    (
        "checkout-api-5b8d44",
        "payments",
        "Running",
        "",
        0,
        "node-02",
    ),
    ("fraud-score-84d7c", "payments", "Running", "", 0, "node-11"),
    (
        "payments-worker-66c9f",
        "payments",
        "Running",
        "",
        1,
        "node-04",
    ),
    (
        "ledger-sync-2884173",
        "payments",
        "Terminated",
        "Completed",
        0,
        "node-09",
    ),
    (
        "catalog-api-6d5f8b",
        "storefront",
        "Running",
        "",
        0,
        "node-03",
    ),
    (
        "image-resizer-59c7d",
        "storefront",
        "Waiting",
        "ImagePullBackOff",
        0,
        "node-12",
    ),
    (
        "search-indexer-7c6f9",
        "storefront",
        "Running",
        "",
        2,
        "node-06",
    ),
    (
        "ingress-nginx-controller",
        "ingress",
        "Running",
        "",
        0,
        "node-01",
    ),
    (
        "otel-collector-gateway",
        "observability",
        "Running",
        "",
        0,
        "node-05",
    ),
];
const POD_SUFFIXES: [&str; 6] = ["x2k9q", "m4tzp", "qw8rn", "h7vcd", "wz2mf", "tb6vn"];
const LOG_LINES: [&str; 12] = [
    "INFO starting checkout-api v2.31.0",
    "INFO loaded config from /etc/checkout/config.yaml",
    "WARN POSTGRES_URL uses a deprecated host, retrying with replica",
    "ERROR dial tcp 10.4.12.8:5432: connect: connection refused",
    "ERROR failed to run migrations: no database connection",
    "INFO shutting down after fatal error",
    "INFO GET /healthz 200 3ms",
    "INFO POST /v1/charges 201 84ms request_id=7be2c1",
    "WARN slow query on orders_by_customer took 1204ms",
    "INFO GET /v1/orders/18274 200 12ms",
    "INFO connected to postgres at 10.4.12.9:5432",
    "INFO flushed 412 spans to otel-collector",
];

fn pod_list(created: &str) -> String {
    let mut items = Vec::new();
    for (name, namespace, state, reason, restarts, node) in WORKLOADS {
        for suffix in POD_SUFFIXES {
            let phase = if state == "Terminated" {
                "Succeeded"
            } else {
                "Running"
            };
            let ready = state == "Running";
            let state = match state {
                "Running" => format!(r#"{{"running":{{"startedAt":"{created}"}}}}"#),
                "Waiting" => format!(r#"{{"waiting":{{"reason":"{reason}"}}}}"#),
                _ => format!(r#"{{"terminated":{{"exitCode":0,"reason":"{reason}"}}}}"#),
            };
            items.push(format!(
                r#"{{"metadata":{{"namespace":"{namespace}","name":"{name}-{suffix}","resourceVersion":"1","creationTimestamp":"{created}"}},"spec":{{"nodeName":"{node}","containers":[{{"name":"app"}}]}},"status":{{"phase":"{phase}","containerStatuses":[{{"name":"app","image":"app","imageID":"app","ready":{ready},"restartCount":{restarts},"state":{state}}}]}}}}"#
            ));
        }
    }
    format!(
        r#"{{"kind":"PodList","apiVersion":"v1","metadata":{{"resourceVersion":"1"}},"items":[{}]}}"#,
        items.join(",")
    )
}

fn log_body() -> String {
    let mut body = String::new();
    for (ix, line) in LOG_LINES.iter().cycle().take(40).enumerate() {
        let (minute, second) = (42 + ix / 60, ix % 60);
        writeln!(body, "2026-11-07T10:{minute:02}:{second:02}.118Z {line}").unwrap();
    }
    body
}

struct Scene {
    mode: ThemeMode,
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    root: Entity<Root>,
}

impl Scene {
    fn open(contexts: Result<Contexts, ClusterError>, mode: ThemeMode) -> Self {
        let platform = gpui_kit::platform::current_platform(true);
        let mut cx = HeadlessAppContext::with_platform(
            platform.text_system(),
            Arc::new(AllAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            Theme::change(mode, None, cx);
        });
        let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(WINDOW.0), px(WINDOW.1)),
            })),
            focus: false,
            show: false,
            ..WindowOptions::default()
        };
        let (handle, root) = cx
            .update(|cx| {
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| Root::new(backend, contexts, window, cx))
                })
            })
            .expect("failed to open window");
        Self {
            mode,
            cx,
            handle,
            root,
        }
    }

    fn wait_until(&mut self, mut ready: impl FnMut(&mut Window, &mut App) -> bool) {
        for _ in 0..WAIT_STEPS {
            self.cx.run_until_parked();
            self.cx.advance_clock(WAIT_STEP);
            let handle = self.handle;
            let done = self
                .cx
                .update_window(handle, |_, window, cx| {
                    window.render_frame(cx);
                    ready(window, cx)
                })
                .expect("preview window closed");
            if done {
                return;
            }
            thread::sleep(WAIT_STEP);
        }
        panic!("preview scene did not settle");
    }

    fn settle(&mut self) {
        let mut frames = 0;
        self.wait_until(|_, _| {
            frames += 1;
            frames > SETTLE_FRAMES
        });
    }

    fn save(&mut self, directory: &Path, name: &str) {
        let (handle, mode) = (self.handle, self.mode);
        // The headless window always reports a light system, which resets the theme on open.
        self.cx.update(|cx| Theme::change(mode, None, cx));
        self.settle();
        let path = directory.join(name);
        self.cx
            .capture_screenshot(handle)
            .expect("failed to render offscreen")
            .save(&path)
            .expect("failed to write preview image");
        println!("{}", path.display());
    }
}

fn connected(server: &str, mode: ThemeMode) -> Scene {
    let contexts = Contexts::from_yaml(&kubeconfig(server, "dev"));
    let mut scene = Scene::open(contexts, mode);
    scene.wait_until(|window, _| {
        window
            .find("status")
            .label()
            .is_some_and(|status| status.contains("Pods:"))
    });
    scene
}

fn tailing(server: &str, mode: ThemeMode) -> Scene {
    let mut scene = connected(server, mode);
    let root = scene.root.clone();
    scene
        .cx
        .update(|cx| root.update(cx, |root, cx| root.tail_logs(cx)));
    scene.wait_until(|window, _| {
        window
            .find("log-summary")
            .label()
            .is_some_and(|summary| !summary.ends_with(" 0 lines kept"))
    });
    scene
}

/// Drives the real app through its main states and writes each one to `directory`.
pub fn run(directory: PathBuf) {
    std::fs::create_dir_all(&directory).expect("failed to create preview directory");
    let created = "2026-11-05T08:00:00Z";
    let server = serve(Fixture {
        pod_list: pod_list(created),
        logs: log_body(),
    });
    assert!(WORKLOADS.len() * POD_SUFFIXES.len() > MAX_TAIL_PODS);

    tailing(&server, ThemeMode::Dark).save(&directory, "logs-dark.png");
    tailing(&server, ThemeMode::Light).save(&directory, "logs-light.png");
    connected(&server, ThemeMode::Dark).save(&directory, "pods-dark.png");

    let mut collapsed = connected(&server, ThemeMode::Light);
    let root = collapsed.root.clone();
    collapsed
        .cx
        .update(|cx| root.update(cx, |root, cx| root.set_sidebar_collapsed(true, cx)));
    collapsed.save(&directory, "collapsed-light.png");

    let mut picker = connected(&server, ThemeMode::Dark);
    let handle = picker.handle;
    picker
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("context-picker", cx);
        })
        .expect("preview window closed");
    picker.save(&directory, "context-menu-dark.png");

    let unreachable = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "dev"));
    let mut failed = Scene::open(unreachable, ThemeMode::Dark);
    failed.wait_until(|window, _| window.try_find("notice-action").is_some());
    failed.save(&directory, "failed-dark.png");

    Scene::open(Contexts::from_yaml("{}"), ThemeMode::Dark)
        .save(&directory, "no-clusters-dark.png");
}
