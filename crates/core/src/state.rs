use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const DIRECTORY: &str = "taffrail";
const FILE: &str = "state.json";

/// What the app restores on startup. It holds nothing from a cluster except context and namespace names.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct SavedState {
    pub window: Option<WindowRect>,
    /// The UUID of the window's display, because macOS reports positions relative to it.
    pub display: Option<String>,
    pub sidebar_collapsed: bool,
    pub context: Option<String>,
    pub namespace: Option<String>,
    /// The pod table's hidden columns.
    pub hidden_columns: Vec<String>,
    /// The resource view last open, such as `Deployments`.
    pub view: Option<String>,
    /// Hidden columns of the other resource views, by view.
    pub view_columns: BTreeMap<String, Vec<String>>,
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct WindowRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The state file under the platform config directory, if the platform has one.
pub fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join(DIRECTORY).join(FILE))
}

impl SavedState {
    /// Reads `path`, falling back to defaults so a bad file never stops the app starting.
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Replaces `path` in one step, so a crash cannot leave half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other(format!("No parent directory: {}", path.display())))?;
        fs::create_dir_all(directory)?;
        let mut file = NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.persist(path)?;
        Ok(())
    }
}

impl WindowRect {
    /// Moves and resizes this rectangle to fit inside `display`. `minimum` wins if the
    /// display is smaller than it.
    pub fn clamped(self, display: Self, minimum: (f32, f32)) -> Self {
        let width = self.width.min(display.width).max(minimum.0);
        let height = self.height.min(display.height).max(minimum.1);
        Self {
            x: self.x.min(display.x + display.width - width).max(display.x),
            y: self
                .y
                .min(display.y + display.height - height)
                .max(display.y),
            width,
            height,
        }
    }

    /// Steps down and right by `offset`, shrinking to stay inside `display`, so a window that
    /// fills the display can still be told apart from the one it was opened from.
    pub fn cascaded(self, display: Self, offset: f32) -> Self {
        let (x, y) = (self.x + offset, self.y + offset);
        Self {
            x,
            y,
            width: self.width.min(display.x + display.width - x),
            height: self.height.min(display.y + display.height - y),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::tempdir;

    use super::{SavedState, WindowRect};

    const DISPLAY: WindowRect = WindowRect {
        x: 0.,
        y: 25.,
        width: 1440.,
        height: 875.,
    };
    const MINIMUM: (f32, f32) = (760., 480.);

    fn rect(x: f32, y: f32, width: f32, height: f32) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
        }
    }

    fn state() -> SavedState {
        SavedState {
            window: Some(rect(10., 20., 900., 700.)),
            display: Some("display-1".into()),
            sidebar_collapsed: true,
            context: Some("staging".into()),
            namespace: Some("payments".into()),
            hidden_columns: vec!["Node".into()],
            view: Some("Deployments".into()),
            view_columns: BTreeMap::from([("Deployments".into(), vec!["Age".into()])]),
        }
    }

    #[test]
    fn saved_state_loads_back_unchanged() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("taffrail").join("state.json");

        state().save(&path).unwrap();

        assert_eq!(SavedState::load(&path), state());
    }

    #[test]
    fn saving_again_replaces_the_file_and_leaves_no_temporary_files() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");

        state().save(&path).unwrap();
        SavedState::default().save(&path).unwrap();

        assert_eq!(SavedState::load(&path), SavedState::default());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_missing_file_gives_defaults() {
        let directory = tempdir().unwrap();

        let loaded = SavedState::load(&directory.path().join("state.json"));

        assert_eq!(loaded, SavedState::default());
    }

    #[test]
    fn a_corrupt_file_gives_defaults() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, "{\"window\": ").unwrap();

        assert_eq!(SavedState::load(&path), SavedState::default());
    }

    #[test]
    fn a_field_of_the_wrong_type_gives_defaults() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"sidebar_collapsed": "yes"}"#).unwrap();

        assert_eq!(SavedState::load(&path), SavedState::default());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"context": "dev", "added_later": [1, 2]}"#).unwrap();

        let loaded = SavedState::load(&path);

        assert_eq!(loaded.context.as_deref(), Some("dev"));
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"sidebar_collapsed": true}"#).unwrap();

        let loaded = SavedState::load(&path);

        assert!(loaded.sidebar_collapsed);
        assert_eq!(loaded.window, None);
        assert_eq!(loaded.context, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_saved_file_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let path = directory.path().join("taffrail").join("state.json");

        state().save(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_failed_save_returns_an_error() {
        let directory = tempdir().unwrap();
        let blocker = directory.path().join("file");
        fs::write(&blocker, "").unwrap();

        let result = state().save(&blocker.join("state.json"));

        assert!(result.is_err());
    }

    #[test]
    fn a_rectangle_inside_the_display_stays_put() {
        let window = rect(100., 100., 900., 700.);

        assert_eq!(window.clamped(DISPLAY, MINIMUM), window);
    }

    #[test]
    fn a_rectangle_off_any_edge_moves_back_inside() {
        let size = (900., 700.);
        let cases = [
            (rect(-5000., 100., 900., 700.), (0., 100.)),
            (rect(5000., 100., 900., 700.), (540., 100.)),
            (rect(100., -5000., 900., 700.), (100., 25.)),
            (rect(100., 5000., 900., 700.), (100., 200.)),
        ];

        for (window, origin) in cases {
            assert_eq!(
                window.clamped(DISPLAY, MINIMUM),
                rect(origin.0, origin.1, size.0, size.1)
            );
        }
    }

    #[test]
    fn a_rectangle_larger_than_the_display_shrinks_to_it() {
        let window = rect(0., 0., 5000., 3000.);

        assert_eq!(window.clamped(DISPLAY, MINIMUM), DISPLAY);
    }

    #[test]
    fn a_rectangle_below_the_minimum_grows_to_it() {
        let window = rect(100., 100., 10., -50.);

        assert_eq!(
            window.clamped(DISPLAY, MINIMUM),
            rect(100., 100., 760., 480.)
        );
    }

    #[test]
    fn the_minimum_wins_on_a_display_smaller_than_it() {
        let small = rect(0., 0., 640., 400.);

        assert_eq!(
            rect(50., 50., 900., 700.).clamped(small, MINIMUM),
            rect(0., 0., 760., 480.)
        );
    }

    #[test]
    fn an_out_of_range_rectangle_still_clamps_to_finite_values() {
        let window = rect(f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, 700.);

        assert_eq!(window.clamped(DISPLAY, MINIMUM), rect(0., 25., 1440., 700.));
    }

    #[test]
    fn cascading_keeps_the_size_when_it_fits() {
        assert_eq!(
            rect(100., 100., 900., 700.).cascaded(DISPLAY, 28.),
            rect(128., 128., 900., 700.)
        );
    }

    #[test]
    fn cascading_a_window_that_fills_the_display_shrinks_it() {
        assert_eq!(DISPLAY.cascaded(DISPLAY, 28.), rect(28., 53., 1412., 847.));
    }
}
