use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::APP_NAME;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// The file the release workflow ships next to the app, listing every crate in the binary.
const NOTICES_FILE: &str = "THIRD-PARTY-LICENSES.html";

/// Where each package puts [`NOTICES_FILE`], relative to the directory holding the binary:
/// next to it on Windows and in the `.tar.gz`, in `Contents/Resources` in the Mac app, and in
/// `/usr/share/doc/taffrail` in the `.deb` and the AppImage.
const NOTICES_DIRS: [&str; 3] = [".", "../Resources", "../share/doc/taffrail"];

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
                .role(Role::Label)
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
        .child(
            Button::new("about-notices")
                .link()
                .small()
                .label("Third-party licences")
                .on_click(|_, _, cx| open_notices(cx)),
        )
}

/// Opens the shipped notices file, or the project license online when the app runs from a
/// build that has none, such as `cargo run`.
fn open_notices(cx: &App) {
    let shipped = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().and_then(notices_path));
    match shipped {
        Some(path) => cx.open_with_system(&path),
        None => cx.open_url(&license_url()),
    }
}

fn notices_path(exe_dir: &Path) -> Option<PathBuf> {
    NOTICES_DIRS
        .iter()
        .map(|dir| exe_dir.join(dir).join(NOTICES_FILE))
        .find(|path| path.is_file())
}

fn license_url() -> String {
    format!("{REPOSITORY}/blob/main/LICENSE")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{NOTICES_FILE, license_url, notices_path};

    #[test]
    fn finds_the_notices_in_each_package_layout() {
        for (exe_dir, notices_dir) in [
            ("taffrail", "taffrail"),
            (
                "Taffrail.app/Contents/MacOS",
                "Taffrail.app/Contents/Resources",
            ),
            ("usr/bin", "usr/share/doc/taffrail"),
        ] {
            let root = tempdir().unwrap();
            let exe_dir = root.path().join(exe_dir);
            let notices = root.path().join(notices_dir).join(NOTICES_FILE);
            fs::create_dir_all(&exe_dir).unwrap();
            fs::create_dir_all(notices.parent().unwrap()).unwrap();
            fs::write(&notices, "").unwrap();

            let found = notices_path(&exe_dir).unwrap();
            assert_eq!(
                found.canonicalize().unwrap(),
                notices.canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn falls_back_to_the_project_license_without_the_notices() {
        let root = tempdir().unwrap();
        assert_eq!(notices_path(root.path()), None);
        assert_eq!(
            license_url(),
            "https://github.com/opikdev/taffrail/blob/main/LICENSE"
        );
    }
}
