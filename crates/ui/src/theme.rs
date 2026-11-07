use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeRegistry};

const THEMES: &str = include_str!("../themes/taffrail.json");
const LIGHT: &str = "Taffrail Light";
const DARK: &str = "Taffrail Dark";

/// Installs both Taffrail themes and selects the one matching the system.
pub fn init(cx: &mut App) {
    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(THEMES)
        .expect("bundled themes are valid");
    let themes = ThemeRegistry::global(cx).themes();
    let light = themes[LIGHT].clone();
    let dark = themes[DARK].clone();
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::sync_system_appearance(None, cx);
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{Hsla, TestAppContext};

    const MINIMUM_CONTRAST: f32 = 4.5;

    fn luminance(color: Hsla) -> f32 {
        let linear = |channel: f32| {
            if channel <= 0.03928 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        let rgb = color.to_rgb();
        0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn assert_readable(mode: ThemeMode, cx: &mut TestAppContext) {
        cx.update(|cx| {
            Theme::change(mode, None, cx);
            let theme = Theme::global(cx);
            let surfaces = [
                theme.sidebar,
                theme.background,
                theme.popover,
                theme.table_active,
                theme.sidebar_accent,
            ];
            let text = [
                theme.foreground,
                theme.muted_foreground,
                theme.danger,
                theme.warning,
                theme.success,
                theme.info,
                theme.ring,
            ];
            for surface in surfaces {
                for color in text {
                    let ratio = contrast(color, surface);
                    assert!(
                        ratio >= MINIMUM_CONTRAST,
                        "{color:?} on {surface:?} measures {ratio:.2}:1 in {mode:?}"
                    );
                }
            }
        });
    }

    #[gpui_kit::test]
    fn every_text_colour_is_readable_on_every_surface(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::init(cx);
        });
        assert_readable(ThemeMode::Light, cx);
        assert_readable(ThemeMode::Dark, cx);
    }

    #[test]
    fn contrast_of_black_on_white_is_twenty_one() {
        let ratio = contrast(gpui_kit::black(), gpui_kit::white());
        assert!((ratio - 21.).abs() < 0.01, "{ratio}");
    }
}
