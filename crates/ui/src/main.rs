#[cfg(feature = "bench")]
mod bench;
#[cfg(any(test, feature = "preview"))]
mod fake_api;
mod logs;
mod pods;
#[cfg(feature = "preview")]
mod preview;
mod root;
mod sidebar;
mod theme;

use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use gpui_kit::*;
use root::Root;
use taffrail_core::{APP_NAME, Backend, Contexts};

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
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    ..root::titlebar_options()
                }),
                app_owns_titlebar_drag: true,
                window_min_size: Some(size(px(760.), px(480.))),
                ..WindowOptions::default()
            };
            #[cfg(feature = "bench")]
            let options = bench::unthrottled(options);
            let window = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Root::new(backend, Contexts::load(), window, cx))
            })
            .expect("failed to open window");
            #[cfg(feature = "bench")]
            bench::run(window, cx);
            #[cfg(not(feature = "bench"))]
            drop(window);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}
