use gpui_kit::*;
use taffrail_core::APP_NAME;

struct Root;

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("root")
            .test_support()
            .aria_label(APP_NAME)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(APP_NAME)
    }
}

fn open_root(cx: &mut App) -> AnyWindowHandle {
    let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Root))
        .expect("failed to open window");
    handle
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        open_root(cx);
    });
}

#[cfg(test)]
mod tests {
    use super::open_root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, TestAppContext};

    #[gpui_kit::test]
    fn renders_the_app_name(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let handle = cx.update(open_root);

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("root").label(), Some("Taffrail"));
        })
        .expect("window should stay open");
    }
}
