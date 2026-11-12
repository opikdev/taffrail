use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use taffrail_core::{APP_ID, APP_NAME, Backend, Contexts, SavedState};

use crate::fake_api::{kubeconfig, serve};
use crate::preview::{README_NAMESPACE, readme_fixture};
use crate::root::Root;
use crate::{root, theme, windows};

const CONTEXT: &str = "dev";
const POLL: Duration = Duration::from_millis(50);

/// Opens the app in a real window on the README screenshot's sample cluster, served from
/// this process, in dark mode. It reads no kubeconfig and restores or saves no state.
pub fn run() {
    let server = serve(readme_fixture());
    let contexts = move || Contexts::from_yaml(&kubeconfig(&server, CONTEXT));
    let backend = Arc::new(Backend::new().expect("failed to start the cluster runtime"));
    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            cx.set_app_identity(APP_ID, APP_NAME);
            cx.set_window_appearance(Some(WindowAppearance::Dark));
            gpui_kit::init(cx);
            theme::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            root::init(cx);
            let saved = windows::init(backend.clone(), None, contexts.clone(), cx);
            let saved = SavedState {
                context: Some(CONTEXT.into()),
                ..saved
            };
            let (window, root) =
                windows::open(backend, contexts(), saved, cx).expect("failed to open window");
            show_readme_state(window, root, cx);
            cx.activate(true);
        });
}

/// Once the pods have loaded, picks the README namespace, tails its pods and opens the first.
fn show_readme_state(window: AnyWindowHandle, root: Entity<Root>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while !cx.update(|cx| root.read(cx).synced_row_count(cx).is_some()) {
            cx.background_executor().timer(POLL).await;
        }
        window
            .update(cx, |_, window, cx| {
                root.update(cx, |root, cx| {
                    root.set_namespace(Some(README_NAMESPACE.into()), window, cx);
                    root.tail_logs(cx);
                    root.select_first_row(cx);
                });
            })
            .ok();
    })
    .detach();
}
