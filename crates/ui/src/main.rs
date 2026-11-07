mod logs;
mod pods;
mod root;

use std::sync::Arc;

use gpui_kit::*;
use root::Root;
use taffrail_core::{APP_NAME, Backend, Contexts};

fn main() {
    let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(APP_NAME.into()),
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Root::new(backend, Contexts::load(), window, cx))
        })
        .expect("failed to open window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
