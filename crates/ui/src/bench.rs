use std::env;
use std::process;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::root::Root;

const HIDDEN_WINDOW: &str =
    "Invalid run: frames stopped. Check the connection, and keep the window uncovered";
const SECONDS_VARIABLE: &str = "TAFFRAIL_BENCH_SECONDS";
const TAIL_VARIABLE: &str = "TAFFRAIL_BENCH_TAIL";
const DEFAULT_SECONDS: u64 = 30;
const ROWS_PER_FRAME: usize = 3;
const READY_POLL: Duration = Duration::from_millis(100);
const SETTLE: Duration = Duration::from_secs(5);
const STARTUP_LIMIT: Duration = Duration::from_secs(60);
const LONGEST_VALID_FRAME: Duration = Duration::from_secs(1);

struct Run {
    root: Entity<Root>,
    rows: usize,
    row: usize,
    last_frame: Instant,
    deadline: Instant,
    intervals: Vec<Duration>,
}

/// GPUI lowers the frame rate of unfocused windows, which would measure the limit
/// rather than the rendering.
pub fn unthrottled(options: WindowOptions) -> WindowOptions {
    WindowOptions {
        inactive_frame_interval: None,
        low_power_frame_interval: None,
        thermal_frame_interval: None,
        ..options
    }
}

/// Tails logs and scrolls the pod table every frame, then prints frame times and quits.
pub fn run((window, root): (AnyWindowHandle, Entity<Root>), cx: &mut App) {
    let seconds = env::var(SECONDS_VARIABLE)
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(DEFAULT_SECONDS);
    // Only presented frames end a run, and macOS stops them for a covered window.
    let limit = cx
        .background_executor()
        .timer(STARTUP_LIMIT + Duration::from_secs(seconds));
    cx.background_executor()
        .spawn(async move {
            limit.await;
            eprintln!("{HIDDEN_WINDOW}");
            process::exit(1);
        })
        .detach();
    cx.spawn(async move |cx| {
        let rows = loop {
            let ready = root.read_with(cx, |root, cx| root.synced_pod_count(cx));
            if let Some(rows) = ready.filter(|rows| *rows > 0) {
                break rows;
            }
            cx.background_executor().timer(READY_POLL).await;
        };
        if env::var(TAIL_VARIABLE).as_deref() != Ok("0") {
            root.update(cx, |root, cx| root.tail_logs(cx));
        }
        cx.background_executor().timer(SETTLE).await;
        window
            .update(cx, |_, window, cx| {
                let now = Instant::now();
                measure_next_frame(
                    Run {
                        root,
                        rows,
                        row: 0,
                        last_frame: now,
                        deadline: now + Duration::from_secs(seconds),
                        intervals: Vec::new(),
                    },
                    window,
                    cx,
                );
            })
            .expect("bench window should stay open");
    })
    .detach();
}

fn measure_next_frame(mut run: Run, window: &mut Window, _: &mut App) {
    window.on_next_frame(move |window, cx| {
        let now = Instant::now();
        run.intervals.push(now - run.last_frame);
        run.last_frame = now;
        if now >= run.deadline {
            if run
                .intervals
                .iter()
                .any(|interval| *interval > LONGEST_VALID_FRAME)
            {
                eprintln!("{HIDDEN_WINDOW}");
                process::exit(1);
            }
            println!("{}", report(run.intervals));
            cx.quit();
            return;
        }
        run.row = (run.row + ROWS_PER_FRAME) % run.rows;
        let row = run.row;
        run.root.update(cx, |root, cx| root.scroll_pods_to(row, cx));
        measure_next_frame(run, window, cx);
    });
}

fn report(mut intervals: Vec<Duration>) -> String {
    let frames = intervals.len();
    let over = |limit: u64| {
        intervals
            .iter()
            .filter(|interval| **interval > Duration::from_millis(limit))
            .count()
    };
    let (over_17ms, over_33ms) = (over(17), over(33));
    intervals.sort_unstable();
    let percentile = |percent: usize| intervals[(frames - 1) * percent / 100].as_secs_f64() * 1000.;
    format!(
        r#"{{"frames":{frames},"p50_ms":{:.2},"p95_ms":{:.2},"p99_ms":{:.2},"max_ms":{:.2},"over_17ms":{over_17ms},"over_33ms":{over_33ms}}}"#,
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
    )
}
