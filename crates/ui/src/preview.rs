use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;
use taffrail_core::{Backend, ClusterError, Contexts, MAX_TAIL_PODS, SavedState};

use crate::fake_api::{Fixture, kubeconfig, serve};
use crate::palette::ShowAbout;
use crate::root::Root;

const WINDOW: (f32, f32) = (1360., 860.);
const NARROW_WINDOW: (f32, f32) = (760., 480.);
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

/// Lines in the formats the log dock reads severity from, as one ingress controller pod.
const FORMAT_LINES: [&str; 14] = [
    r#"I1107 10:42:06.118123       7 main.go:104] "Starting NGINX Ingress controller" version="v1.12.1""#,
    r#"{"level":"info","ts":"2026-11-07T10:42:06.2Z","msg":"watching ingresses","namespace":"storefront","class":"nginx"}"#,
    r#"level=debug msg="sync loop" took=12ms"#,
    r#"10.4.3.21 - - [07/Nov/2026:10:42:07 +0000] "GET /v1/orders/18274 HTTP/1.1" 200 512 "-" "curl/8.7.1""#,
    r#"W1107 10:42:07.402911       7 controller.go:1126] Service "storefront/catalog" does not have any active Endpoint"#,
    r#"{"level":"warn","msg":"slow upstream","upstream":"catalog-api","latency_ms":1204}"#,
    "2026/11/07 10:42:08 [error] 31#31: *4821 connect() failed (111: Connection refused) while connecting to upstream",
    r#"{"level":50,"time":1794048128,"msg":"health check failed","pod":"catalog-api-6d5f8b-x2k9q","attempt":3}"#,
    "ERROR:root:certificate for shop.example.com expires in 6 days",
    r#"time="2026-11-07T10:42:09Z" level=info msg="reloaded configuration" backends=14"#,
    r#"{"severity":"DEBUG","message":"cache hit","key":"ingress/storefront"}"#,
    r#"10.4.3.22 - - [07/Nov/2026:10:42:09 +0000] "POST /v1/charges HTTP/1.1" 201 84 "-" "checkout/2.31""#,
    r#"E1107 10:42:10.118123       7 queue.go:131] "requeuing" err="failed to sync: context deadline exceeded""#,
    r#"{"level":"info","msg":"request finished","method":"GET","path":"/healthz","status":200,"duration":"3ms"}"#,
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
            let app = app_label(name);
            items.push(format!(
                r#"{{"metadata":{{"namespace":"{namespace}","name":"{name}-{suffix}","uid":"{name}-{suffix}","resourceVersion":"1","creationTimestamp":"{created}","labels":{{"app":"{app}"}},"ownerReferences":[{{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"{name}","uid":"{name}","controller":true}}]}},"spec":{{"nodeName":"{node}","containers":[{{"name":"app"}}]}},"status":{{"phase":"{phase}","containerStatuses":[{{"name":"app","image":"app","imageID":"app","ready":{ready},"restartCount":{restarts},"state":{state}}}]}}}}"#
            ));
        }
    }
    format!(
        r#"{{"kind":"PodList","apiVersion":"v1","metadata":{{"resourceVersion":"1"}},"items":[{}]}}"#,
        items.join(",")
    )
}

/// The `app` label of a workload's pods: its name without the ReplicaSet hash.
fn app_label(name: &str) -> &str {
    name.rsplit_once('-')
        .filter(|(_, hash)| hash.len() >= 5 && hash.chars().any(|char| char.is_ascii_digit()))
        .map_or(name, |(app, _)| app)
}

/// The ReplicaSets of the `checkout-api` rollout and their Deployment, for following it.
fn workload_objects() -> Vec<(String, String)> {
    let replica_set = |hash: &str| {
        (
            format!("/apis/apps/v1/namespaces/payments/replicasets/checkout-api-{hash}"),
            format!(
                r#"{{"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{{"name":"checkout-api-{hash}","namespace":"payments","ownerReferences":[{{"apiVersion":"apps/v1","kind":"Deployment","name":"checkout-api","uid":"d","controller":true}}]}},"spec":{{"selector":{{"matchLabels":{{"app":"checkout-api","pod-template-hash":"{hash}"}}}}}}}}"#
            ),
        )
    };
    vec![
        replica_set("7f9c6d"),
        replica_set("5b8d44"),
        (
            "/apis/apps/v1/namespaces/payments/deployments/checkout-api".to_owned(),
            r#"{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"checkout-api","namespace":"payments"},"spec":{"selector":{"matchLabels":{"app":"checkout-api"}},"template":{}}}"#.to_owned(),
        ),
    ]
}

fn pod_body(created: &str) -> String {
    format!(
        r#"{{"kind":"Pod","apiVersion":"v1","metadata":{{"namespace":"payments","name":"checkout-api-7f9c6d-h7vcd","uid":"3f1c9a7e-52ab-4b7e-9f55-0a0f6d1f1a11","resourceVersion":"188274","creationTimestamp":"{created}","labels":{{"app":"checkout-api","pod-template-hash":"7f9c6d","tier":"backend"}},"ownerReferences":[{{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"checkout-api-7f9c6d","uid":"9","controller":true}}],"managedFields":[{{"manager":"kube-controller-manager"}}]}},"spec":{{"nodeName":"node-07","restartPolicy":"Always","serviceAccountName":"checkout-api","containers":[{{"name":"app","image":"registry.example.com/checkout-api:2.31.0","ports":[{{"containerPort":8080,"protocol":"TCP"}}],"env":[{{"name":"POSTGRES_URL","value":"postgres://10.4.12.8:5432/checkout"}}],"resources":{{"limits":{{"memory":"512Mi"}},"requests":{{"cpu":"250m","memory":"256Mi"}}}}}},{{"name":"proxy","image":"registry.example.com/envoy:1.30.2"}}]}},"status":{{"phase":"Running","podIP":"10.4.3.21","hostIP":"10.0.7.4","containerStatuses":[{{"name":"app","image":"checkout-api:2.31.0","imageID":"x","ready":false,"restartCount":7,"state":{{"waiting":{{"reason":"CrashLoopBackOff"}}}},"lastState":{{"terminated":{{"exitCode":1,"reason":"Error"}}}}}},{{"name":"proxy","image":"envoy:1.30.2","imageID":"x","ready":true,"restartCount":0,"state":{{"running":{{"startedAt":"{created}"}}}}}}]}}}}"#
    )
}

fn events_body() -> String {
    let event = |name: &str, kind: &str, reason: &str, message: &str, count: u32, minute: u32| {
        format!(
            r#"{{"metadata":{{"name":"{name}","namespace":"payments"}},"involvedObject":{{"kind":"Pod","name":"pod"}},"type":"{kind}","reason":"{reason}","message":"{message}","count":{count},"lastTimestamp":"2026-11-07T10:{minute:02}:00Z"}}"#
        )
    };
    let items = [
        event(
            "e1",
            "Warning",
            "BackOff",
            "Back-off restarting failed container app in pod checkout-api-7f9c6d-h7vcd",
            41,
            46,
        ),
        event(
            "e2",
            "Normal",
            "Pulled",
            "Container image registry.example.com/checkout-api:2.31.0 already present on machine",
            8,
            42,
        ),
        event("e3", "Normal", "Started", "Started container app", 8, 42),
        event(
            "e4",
            "Warning",
            "Unhealthy",
            "Readiness probe failed: connection refused",
            3,
            41,
        ),
        event(
            "e5",
            "Normal",
            "Scheduled",
            "Successfully assigned payments/checkout-api-7f9c6d-h7vcd to node-07",
            1,
            38,
        ),
    ];
    format!(
        r#"{{"kind":"EventList","apiVersion":"v1","metadata":{{"resourceVersion":"1"}},"items":[{}]}}"#,
        items.join(",")
    )
}

fn log_body() -> String {
    stamped(&LOG_LINES, 40)
}

/// `count` lines from `lines`, one second apart from 10:42.
fn stamped(lines: &[&str], count: usize) -> String {
    stamped_from(lines, count, 0)
}

/// `count` lines from `lines`, one second apart from `first` seconds after 10:42.
fn stamped_from(lines: &[&str], count: usize, first: usize) -> String {
    let mut body = String::new();
    for (ix, line) in lines.iter().cycle().take(count).enumerate() {
        let ix = ix + first;
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
        Self::open_at(contexts, mode, WINDOW)
    }

    fn open_at(
        contexts: Result<Contexts, ClusterError>,
        mode: ThemeMode,
        (width, height): (f32, f32),
    ) -> Self {
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
            crate::root::init(cx);
            Theme::change(mode, None, cx);
        });
        let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(width), px(height)),
            })),
            focus: false,
            show: false,
            ..WindowOptions::default()
        };
        let (handle, root) = cx
            .update(|cx| {
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| Root::new(backend, contexts, SavedState::default(), window, cx))
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

    fn set_namespace(&mut self, namespace: &str) {
        let (handle, root) = (self.handle, self.root.clone());
        self.cx
            .update_window(handle, |_, window, cx| {
                root.update(cx, |root, cx| {
                    root.set_namespace(Some(namespace.into()), window, cx);
                });
            })
            .expect("preview window closed");
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
    connected_at(server, mode, WINDOW)
}

fn connected_at(server: &str, mode: ThemeMode, window: (f32, f32)) -> Scene {
    let contexts = Contexts::from_yaml(&kubeconfig(server, "dev"));
    let mut scene = Scene::open_at(contexts, mode, window);
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
        pod: pod_body(created),
        events: events_body(),
        logs: log_body(),
        live_logs: None,
        reconnect_logs: None,
        objects: workload_objects(),
    });
    assert!(WORKLOADS.len() * POD_SUFFIXES.len() > MAX_TAIL_PODS);

    let live = serve(Fixture {
        pod_list: pod_list(created),
        pod: pod_body(created),
        events: events_body(),
        logs: String::new(),
        live_logs: Some(&LOG_LINES),
        reconnect_logs: None,
        objects: workload_objects(),
    });
    let mut readme = connected(&live, ThemeMode::Dark);
    readme.set_namespace("payments");
    let root = readme.root.clone();
    readme.cx.update(|cx| {
        root.update(cx, |root, cx| {
            root.tail_logs(cx);
            root.select_first_row(cx);
        })
    });
    readme.wait_until(|window, _| {
        window.try_find("details-title").is_some()
            && window
                .find("log-summary")
                .label()
                .is_some_and(|summary| !summary.ends_with(" 0 lines kept"))
    });
    readme.save(&directory, "readme-dark.png");
    drop(readme);

    tailing(&server, ThemeMode::Dark).save(&directory, "logs-dark.png");
    tailing(&server, ThemeMode::Light).save(&directory, "logs-light.png");
    connected(&server, ThemeMode::Dark).save(&directory, "pods-dark.png");

    let formats = serve(Fixture {
        pod_list: pod_list(created),
        pod: pod_body(created),
        events: events_body(),
        logs: stamped(&FORMAT_LINES, FORMAT_LINES.len()),
        live_logs: None,
        reconnect_logs: None,
        objects: workload_objects(),
    });
    for (mode, name) in [
        (ThemeMode::Dark, "log-formats-dark.png"),
        (ThemeMode::Light, "log-formats-light.png"),
    ] {
        let mut scene = connected(&formats, mode);
        scene.set_namespace("ingress");
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
        scene.save(&directory, name);
    }

    for (mode, name) in [
        (ThemeMode::Dark, "log-gap-dark.png"),
        (ThemeMode::Light, "log-gap-light.png"),
    ] {
        // The log rotates while the stream is down, so lines from 10:42:05 to 10:42:31
        // are lost. A server serves one scene, because it counts each pod's requests.
        let rotated = serve(Fixture {
            pod_list: pod_list(created),
            pod: pod_body(created),
            events: events_body(),
            logs: stamped(&FORMAT_LINES, 6),
            live_logs: None,
            objects: workload_objects(),
            reconnect_logs: Some(stamped_from(&FORMAT_LINES[6..], 8, 31)),
        });
        let mut scene = connected(&rotated, mode);
        scene.set_namespace("ingress");
        let root = scene.root.clone();
        scene
            .cx
            .update(|cx| root.update(cx, |root, cx| root.tail_logs(cx)));
        scene.wait_until(|window, _| {
            window
                .find("log-summary")
                .label()
                .and_then(|summary| summary.strip_suffix(" lines kept")?.rsplit(' ').next())
                .and_then(|kept| kept.replace(',', "").parse::<usize>().ok())
                // Each pod's first lines, two notes, the gap and the lines after it.
                .is_some_and(|kept| kept >= POD_SUFFIXES.len() * 17)
        });
        scene.save(&directory, name);
    }

    let mut collapsed = connected(&server, ThemeMode::Light);
    let root = collapsed.root.clone();
    collapsed
        .cx
        .update(|cx| root.update(cx, |root, cx| root.set_sidebar_collapsed(true, cx)));
    collapsed.save(&directory, "collapsed-light.png");

    let mut details = tailing(&server, ThemeMode::Dark);
    let root = details.root.clone();
    details
        .cx
        .update(|cx| root.update(cx, |root, cx| root.select_first_row(cx)));
    details.wait_until(|window, _| window.try_find("details-title").is_some());
    details.settle();
    details.save(&directory, "details-summary-dark.png");
    let handle = details.handle;
    details
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("details-tabs").click(1usize, cx);
        })
        .expect("preview window closed");
    details.save(&directory, "details-events-dark.png");
    details
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("details-tabs").click(2usize, cx);
        })
        .expect("preview window closed");
    details.save(&directory, "details-yaml-dark.png");

    let mut follow = connected(&server, ThemeMode::Dark);
    let root = follow.root.clone();
    follow.cx.update(|cx| {
        root.update(cx, |root, cx| {
            root.select_first_row(cx);
            root.tail_workload(cx);
        })
    });
    follow.wait_until(|window, _| {
        window.find("log-summary").label().is_some_and(|summary| {
            summary.starts_with("Following deployment/checkout-api")
                && !summary.ends_with(" 0 lines kept")
        })
    });
    follow.save(&directory, "follow-dark.png");

    let mut palette = connected(&server, ThemeMode::Dark);
    let handle = palette.handle;
    palette
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("open-palette", cx);
        })
        .expect("preview window closed");
    palette.save(&directory, "palette-dark.png");
    palette
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.input("checkout", cx);
        })
        .expect("preview window closed");
    palette.save(&directory, "palette-search-dark.png");

    let mut shortcuts = connected(&server, ThemeMode::Light);
    let handle = shortcuts.handle;
    shortcuts
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("?", cx);
        })
        .expect("preview window closed");
    shortcuts.save(&directory, "shortcuts-light.png");

    for (mode, name) in [
        (ThemeMode::Dark, "about-dark.png"),
        (ThemeMode::Light, "about-light.png"),
    ] {
        let mut about = connected(&server, mode);
        let handle = about.handle;
        about
            .cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.dispatch_action(Box::new(ShowAbout), cx);
            })
            .expect("preview window closed");
        about.save(&directory, name);
    }

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

    let mut namespaces = connected(&server, ThemeMode::Dark);
    namespaces.set_namespace("payments");
    namespaces.save(&directory, "namespace-dark.png");
    let handle = namespaces.handle;
    namespaces
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("namespace-picker").click("input", cx);
        })
        .expect("preview window closed");
    namespaces.save(&directory, "namespace-menu-dark.png");

    let mut columns = connected(&server, ThemeMode::Dark);
    let root = columns.root.clone();
    columns.cx.update(|cx| {
        root.update(cx, |root, cx| {
            root.set_hidden_columns(["Namespace", "Node"], cx);
        })
    });
    columns.save(&directory, "columns-hidden-dark.png");
    let handle = columns.handle;
    columns
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("columns", cx);
        })
        .expect("preview window closed");
    columns.save(&directory, "columns-menu-dark.png");

    let mut narrow = connected_at(&server, ThemeMode::Dark, NARROW_WINDOW);
    narrow.save(&directory, "narrow-dark.png");
    let handle = narrow.handle;
    narrow
        .cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("columns", cx);
        })
        .expect("preview window closed");
    narrow.save(&directory, "narrow-columns-menu-dark.png");

    let mut empty = connected(&server, ThemeMode::Light);
    empty.set_namespace("billing");
    empty.save(&directory, "no-namespace-pods-light.png");

    let unreachable = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "dev"));
    let mut failed = Scene::open(unreachable, ThemeMode::Dark);
    failed.wait_until(|window, _| window.try_find("notice-action").is_some());
    failed.save(&directory, "failed-dark.png");

    Scene::open(Contexts::from_yaml("{}"), ThemeMode::Dark)
        .save(&directory, "no-clusters-dark.png");
}
