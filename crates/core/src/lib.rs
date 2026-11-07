//! Cluster state, watches and log storage. This crate never imports GPUI.

mod cluster;
mod details;
mod logs;
mod pods;
mod state;

pub use cluster::{Backend, ClusterError, Connection, Contexts};
pub use details::{ContainerDetails, PodDetails, PodEvent};
pub use logs::{LogLine, LogProgress, LogReader, LogTail, LogTarget, MAX_TAIL_PODS};
pub use pods::{PodKey, PodRow, PodSnapshot, PodWatch, log_targets};
pub use state::{SavedState, WindowRect, state_path};

pub const APP_NAME: &str = "Taffrail";
