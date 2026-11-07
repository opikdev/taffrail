use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use taffrail_core::{APP_NAME, Backend, ClusterError, Contexts, SavedState, WindowRect};

use crate::root::{self, Root};

actions!(taffrail, [NewWindow, Quit]);

const NEW_WINDOW_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-n"
} else {
    "ctrl-n"
};
const QUIT_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-q"
} else {
    "ctrl-q"
};
const MINIMUM_SIZE: (f32, f32) = (760., 480.);
const NEW_WINDOW_OFFSET: f32 = 28.;
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// The state of whichever window changed last, and the file it is saved to.
struct StateFile {
    path: PathBuf,
    state: SavedState,
    pending_save: Option<Task<()>>,
}

impl Global for StateFile {}

/// Adds the menu, keys and quit behaviour, and returns the state to restore. Without a
/// `state_path` nothing is restored or saved.
pub fn init(
    backend: Arc<Backend>,
    state_path: Option<PathBuf>,
    load_contexts: impl Fn() -> Result<Contexts, ClusterError> + 'static,
    cx: &mut App,
) -> SavedState {
    cx.set_quit_mode(QuitMode::LastWindowClosed);
    cx.bind_keys([
        KeyBinding::new(NEW_WINDOW_KEY, NewWindow, None),
        KeyBinding::new(QUIT_KEY, Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(move |_: &NewWindow, cx| {
        let backend = backend.clone();
        let contexts = load_contexts();
        // The active window cannot be read until the action dispatched to it returns.
        cx.defer(move |cx| open_beside_active(backend, contexts, cx));
    });
    cx.set_menus([
        Menu::new(APP_NAME).items([MenuItem::action(format!("Quit {APP_NAME}"), Quit)]),
        Menu::new("File").items([MenuItem::action("New window", NewWindow)]),
    ]);
    let Some(path) = state_path else {
        return SavedState::default();
    };
    let state = SavedState::load(&path);
    cx.set_global(StateFile {
        path,
        state: state.clone(),
        pending_save: None,
    });
    cx.on_window_closed(|cx, _| save_now(cx)).detach();
    cx.on_app_quit(|cx| {
        save_now(cx);
        async {}
    })
    .detach();
    state
}

/// Opens a window showing `saved`, inside the display it was on if that is still connected.
pub fn open(
    backend: Arc<Backend>,
    contexts: Result<Contexts, ClusterError>,
    saved: SavedState,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Root>)> {
    let display = display_for(&saved, cx);
    let window_bounds = saved.window.zip(display.as_ref()).map(|(rect, display)| {
        let visible = to_rect(display.visible_bounds());
        WindowBounds::Windowed(to_bounds(rect.clamped(visible, MINIMUM_SIZE)))
    });
    let options = WindowOptions {
        window_bounds,
        display_id: display.map(|display| display.id()),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_NAME.into()),
            ..root::titlebar_options()
        }),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(MINIMUM_SIZE.0), px(MINIMUM_SIZE.1))),
        ..WindowOptions::default()
    };
    #[cfg(feature = "bench")]
    let options = crate::bench::unthrottled(options);
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Root::new(backend, contexts, saved, window, cx))
    })
}

fn open_beside_active(
    backend: Arc<Backend>,
    contexts: Result<Contexts, ClusterError>,
    cx: &mut App,
) {
    let mut state = cx
        .active_window()
        .or_else(|| cx.windows().first().copied())
        .and_then(|window| window.update(cx, |_, window, cx| state_of(window, cx)).ok())
        .flatten()
        .unwrap_or_default();
    if let (Some(rect), Some(display)) = (state.window, display_for(&state, cx)) {
        let visible = to_rect(display.visible_bounds());
        state.window = Some(rect.cascaded(visible, NEW_WINDOW_OFFSET));
    }
    if let Err(error) = open(backend, contexts, state, cx) {
        eprintln!("Failed to open a new window: {error}");
    }
}

fn state_of(window: &Window, cx: &App) -> Option<SavedState> {
    let view = base::Root::read(window, cx).view().clone();
    let root = view.downcast::<Root>().ok()?;
    Some(root.read(cx).saved_state(window, cx))
}

fn display_for(state: &SavedState, cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
    cx.displays()
        .into_iter()
        .find(|display| state.display.is_some() && display_uuid(display.as_ref()) == state.display)
        .or_else(|| cx.primary_display())
}

fn display_uuid(display: &dyn PlatformDisplay) -> Option<String> {
    display.uuid().ok().map(|uuid| uuid.to_string())
}

/// Adds where `window` is to `state`. A fullscreen window keeps the size it returns to.
pub fn with_placement(state: SavedState, window: &Window, cx: &App) -> SavedState {
    SavedState {
        window: Some(to_rect(window.window_bounds().get_bounds())),
        display: window
            .display(cx)
            .and_then(|display| display_uuid(display.as_ref())),
        ..state
    }
}

/// Saves `state` once changes stop, so dragging a window does not write every frame.
pub fn remember(state: SavedState, cx: &mut App) {
    if cx
        .try_global::<StateFile>()
        .is_none_or(|file| file.state == state)
    {
        return;
    }
    let pending_save = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        cx.update(save_now);
    });
    let file = cx.global_mut::<StateFile>();
    file.state = state;
    file.pending_save = Some(pending_save);
}

// This writes on the UI thread, because a write left running on another thread could land
// after the final save on close and replace it with older state.
fn save_now(cx: &mut App) {
    let file = cx.global_mut::<StateFile>();
    if file.pending_save.take().is_some()
        && let Err(error) = file.state.save(&file.path)
    {
        eprintln!(
            "Failed to save window state to {}: {error}",
            file.path.display()
        );
    }
}

fn to_rect(bounds: Bounds<Pixels>) -> WindowRect {
    WindowRect {
        x: bounds.origin.x.as_f32(),
        y: bounds.origin.y.as_f32(),
        width: bounds.size.width.as_f32(),
        height: bounds.size.height.as_f32(),
    }
}

fn to_bounds(rect: WindowRect) -> Bounds<Pixels> {
    Bounds::new(
        point(px(rect.x), px(rect.y)),
        size(px(rect.width), px(rect.height)),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, px, size};
    use taffrail_core::{Backend, ClusterError, Contexts, SavedState, WindowRect};
    use tempfile::tempdir;

    use super::{NEW_WINDOW_KEY, SAVE_DELAY, open, state_of};
    use crate::root;

    const KUBECONFIG: &str = r#"
current-context: staging
clusters:
  - name: x
    cluster:
      server: http://127.0.0.1:1
contexts:
  - name: staging
    context:
      cluster: x
      user: x
  - name: production
    context:
      cluster: x
      user: x
users:
  - name: x
    user: {}
"#;

    fn contexts() -> Result<Contexts, ClusterError> {
        Contexts::from_yaml(KUBECONFIG)
    }

    fn init(state_path: Option<PathBuf>, cx: &mut TestAppContext) -> Arc<Backend> {
        // Each window really connects, and the answer arrives from a backend thread.
        cx.executor().allow_parking();
        let backend = Arc::new(Backend::new().unwrap());
        let shared = backend.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            root::init(cx);
            super::init(shared, state_path, contexts, cx);
        });
        backend
    }

    fn open_saved(
        backend: &Arc<Backend>,
        saved: SavedState,
        cx: &mut TestAppContext,
    ) -> AnyWindowHandle {
        cx.update(|cx| open(backend.clone(), contexts(), saved, cx).unwrap().0)
    }

    fn state(handle: AnyWindowHandle, cx: &mut TestAppContext) -> SavedState {
        cx.update_window(handle, |_, window, cx| state_of(window, cx).unwrap())
            .unwrap()
    }

    fn toggle_sidebar(handle: AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("toggle-sidebar", cx);
        })
        .unwrap();
    }

    fn resize(handle: AnyWindowHandle, width: f32, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| {
            window.resize(size(px(width), px(600.)));
            window.bounds_changed(cx);
        })
        .unwrap();
    }

    fn wait_out_the_delay(cx: &mut TestAppContext) {
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
    }

    fn collapsed_on_disk(path: &Path) -> bool {
        SavedState::load(path).sidebar_collapsed
    }

    #[gpui_kit::test]
    fn reopens_on_the_saved_context(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            context: Some("production".into()),
            ..SavedState::default()
        };

        let handle = open_saved(&backend, saved, cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("status").label(),
                Some("Connecting to production")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn falls_back_to_the_current_context_when_the_saved_one_is_gone(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            context: Some("deleted".into()),
            ..SavedState::default()
        };

        let handle = open_saved(&backend, saved, cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("status").label(), Some("Connecting to staging"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn keeps_the_saved_context_while_the_kubeconfig_cannot_be_read(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            context: Some("production".into()),
            ..SavedState::default()
        };

        let handle = cx.update(|cx| {
            open(
                backend.clone(),
                Contexts::from_yaml("clusters: ["),
                saved,
                cx,
            )
            .unwrap()
            .0
        });

        assert_eq!(state(handle, cx).context.as_deref(), Some("production"));
    }

    #[gpui_kit::test]
    fn reopens_with_the_saved_sidebar_state(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            sidebar_collapsed: true,
            ..SavedState::default()
        };

        let handle = open_saved(&backend, saved, cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("toggle-sidebar").label(), Some("Show sidebar"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn pulls_restored_bounds_onto_the_display_and_up_to_the_minimum(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            window: Some(WindowRect {
                x: 9000.,
                y: 9000.,
                width: 100.,
                height: 100.,
            }),
            ..SavedState::default()
        };

        let handle = open_saved(&backend, saved, cx);

        assert_eq!(
            state(handle, cx).window,
            Some(WindowRect {
                x: 1160.,
                y: 600.,
                width: 760.,
                height: 480.,
            })
        );
    }

    #[gpui_kit::test]
    fn the_new_window_key_opens_the_same_context_beside_the_window(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let saved = SavedState {
            window: Some(WindowRect {
                x: 100.,
                y: 100.,
                width: 900.,
                height: 700.,
            }),
            sidebar_collapsed: true,
            context: Some("production".into()),
            ..SavedState::default()
        };
        let first = open_saved(&backend, saved, cx);

        cx.update_window(first, |_, window, cx| {
            window.render_frame(cx);
            window.press(NEW_WINDOW_KEY, cx);
        })
        .unwrap();
        cx.run_until_parked();

        let windows = cx.update(|cx| cx.windows());
        assert_eq!(windows.len(), 2);
        let second = state(windows[1], cx);
        assert_eq!(second.context.as_deref(), Some("production"));
        assert!(second.sidebar_collapsed);
        assert_eq!(
            second.window,
            Some(WindowRect {
                x: 128.,
                y: 128.,
                width: 900.,
                height: 700.,
            })
        );
    }

    #[gpui_kit::test]
    fn a_new_window_follows_the_context_chosen_in_the_first(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let first = open_saved(&backend, SavedState::default(), cx);

        cx.update_window(first, |_, window, cx| {
            window.render_frame(cx);
            window.within("context-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.update_window(first, |_, window, cx| {
            window.render_frame(cx);
            window.press(NEW_WINDOW_KEY, cx);
        })
        .unwrap();
        cx.run_until_parked();

        let windows = cx.update(|cx| cx.windows());
        assert_eq!(windows.len(), 2);
        assert_eq!(state(windows[1], cx).context.as_deref(), Some("production"));
    }

    #[gpui_kit::test]
    fn each_window_keeps_its_own_context(cx: &mut TestAppContext) {
        let backend = init(None, cx);
        let staging = open_saved(&backend, SavedState::default(), cx);
        let production = open_saved(
            &backend,
            SavedState {
                context: Some("production".into()),
                ..SavedState::default()
            },
            cx,
        );

        assert_eq!(state(staging, cx).context.as_deref(), Some("staging"));
        assert_eq!(state(production, cx).context.as_deref(), Some("production"));
    }

    #[gpui_kit::test]
    fn saves_once_changes_stop(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("taffrail").join("state.json");
        let backend = init(Some(path.clone()), cx);
        let handle = open_saved(&backend, SavedState::default(), cx);

        toggle_sidebar(handle, cx);
        cx.executor().advance_clock(SAVE_DELAY / 2);
        cx.run_until_parked();
        resize(handle, 1000., cx);
        cx.executor().advance_clock(SAVE_DELAY / 2);
        cx.run_until_parked();
        assert!(!path.exists());

        wait_out_the_delay(cx);

        let saved = SavedState::load(&path);
        assert!(saved.sidebar_collapsed);
        assert_eq!(saved.window.map(|window| window.width), Some(1000.));
        assert!(saved.display.is_some());
    }

    #[gpui_kit::test]
    fn saves_the_chosen_context(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let backend = init(Some(path.clone()), cx);
        let handle = open_saved(&backend, SavedState::default(), cx);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("context-picker").click("input", cx);
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        wait_out_the_delay(cx);

        assert_eq!(
            SavedState::load(&path).context.as_deref(),
            Some("production")
        );
    }

    #[gpui_kit::test]
    fn closing_a_window_saves_at_once(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let backend = init(Some(path.clone()), cx);
        let handle = open_saved(&backend, SavedState::default(), cx);

        toggle_sidebar(handle, cx);
        cx.update_window(handle, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();

        assert!(collapsed_on_disk(&path));
    }

    #[gpui_kit::test]
    fn the_last_window_to_change_wins(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let backend = init(Some(path.clone()), cx);
        let first = open_saved(&backend, SavedState::default(), cx);
        let second = open_saved(&backend, SavedState::default(), cx);

        resize(first, 1000., cx);
        wait_out_the_delay(cx);
        resize(second, 1100., cx);
        wait_out_the_delay(cx);

        let saved = SavedState::load(&path);
        assert_eq!(saved.window.map(|window| window.width), Some(1100.));
    }

    #[gpui_kit::test]
    fn a_bounds_event_without_a_change_writes_nothing(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let backend = init(Some(path.clone()), cx);
        let handle = open_saved(&backend, SavedState::default(), cx);
        resize(handle, 1000., cx);
        wait_out_the_delay(cx);
        fs::remove_file(&path).unwrap();

        resize(handle, 1000., cx);
        wait_out_the_delay(cx);

        assert!(!path.exists());
    }

    #[gpui_kit::test]
    fn a_failed_save_leaves_the_app_running(cx: &mut TestAppContext) {
        let directory = tempdir().unwrap();
        let blocker = directory.path().join("file");
        fs::write(&blocker, "").unwrap();
        let backend = init(Some(blocker.join("state.json")), cx);
        let handle = open_saved(&backend, SavedState::default(), cx);

        toggle_sidebar(handle, cx);
        wait_out_the_delay(cx);

        toggle_sidebar(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("toggle-sidebar").label(), Some("Hide sidebar"));
        })
        .unwrap();
    }
}
