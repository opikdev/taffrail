mod about;
#[cfg(feature = "bench")]
mod bench;
mod daemonsets;
mod deployments;
mod details;
mod events;
#[cfg(any(test, feature = "preview"))]
mod fake_api;
mod jobs;
mod logs;
mod namespaces;
mod nodes;
mod palette;
mod pods;
#[cfg(feature = "preview")]
mod preview;
mod root;
mod rows;
mod sidebar;
mod statefulsets;
mod table;
mod theme;
mod timeline;
mod views;
mod windows;
mod workload;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use taffrail_core::{APP_ID, APP_NAME, Backend, Contexts};

fn main() {
    #[cfg(feature = "preview")]
    if let Some(directory) = std::env::var_os("TAFFRAIL_PREVIEW_DIR") {
        preview::run(directory.into());
        return;
    }
    taffrail_core::use_login_shell_environment();
    let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            cx.set_app_identity(APP_ID, APP_NAME);
            gpui_kit::init(cx);
            theme::init(cx);
            root::init(cx);
            let saved = windows::init(backend.clone(), state_path(), Contexts::load, cx);
            let window =
                windows::open(backend, Contexts::load(), saved, cx).expect("failed to open window");
            #[cfg(feature = "bench")]
            bench::run(window, cx);
            #[cfg(not(feature = "bench"))]
            drop(window);
            cx.activate(true);
        });
}

/// A benchmark must not restore a window size, or connect to the last context.
fn state_path() -> Option<PathBuf> {
    taffrail_core::state_path().filter(|_| !cfg!(feature = "bench"))
}
