use std::sync::{Arc, LazyLock};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::APP_NAME;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");

const ICON_SIZE: f32 = 64.;

static ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../assets/icon/128x128.png").to_vec(),
    ))
});

/// The sheet that **About Taffrail** opens.
pub fn render_about(cx: &App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    let version = format!("Version {VERSION}");
    let license = format!("Licensed under {LICENSE}");
    let repository = REPOSITORY.trim_start_matches("https://");
    v_flex()
        .id("about")
        .test_support()
        .role(Role::Dialog)
        .aria_label(format!("About {APP_NAME}"))
        .items_center()
        .gap_1()
        .p_6()
        .text_sm()
        .child(img(ICON.clone()).size(px(ICON_SIZE)).mb_2())
        .child(
            div()
                .id("about-name")
                .role(Role::Heading)
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(APP_NAME),
        )
        .child(
            div()
                .id("about-version")
                .test_support()
                .aria_label(version.clone())
                .text_color(muted)
                .child(version),
        )
        .child(div().text_color(muted).child(license))
        .child(
            Button::new("about-repository")
                .link()
                .small()
                .mt_2()
                .label(repository)
                .on_click(|_, _, cx| cx.open_url(REPOSITORY)),
        )
}
