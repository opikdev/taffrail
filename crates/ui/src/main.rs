#[cfg(feature = "bench")]
mod bench;
mod details;
#[cfg(any(test, feature = "preview"))]
mod fake_api;
mod logs;
mod pods;
#[cfg(feature = "preview")]
mod preview;
mod root;
mod sidebar;
mod theme;
mod windows;

use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use taffrail_core::{Backend, Contexts, SavedState};

fn main() {
    #[cfg(feature = "preview")]
    if let Some(directory) = std::env::var_os("TAFFRAIL_PREVIEW_DIR") {
        preview::run(directory.into());
        return;
    }
    let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            root::init(cx);
            windows::init(backend.clone(), Contexts::load, cx);
            let window = windows::open(backend, Contexts::load(), SavedState::default(), cx)
                .expect("failed to open window");
            #[cfg(feature = "bench")]
            bench::run(window, cx);
            #[cfg(not(feature = "bench"))]
            drop(window);
            cx.activate(true);
        });
}
