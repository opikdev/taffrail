//! Cluster state, watches and log storage. This crate never imports GPUI.

mod cluster;
mod daemonsets;
mod deployments;
mod details;
mod environment;
#[cfg(test)]
mod fake_api;
mod follow;
mod jobs;
#[cfg(test)]
mod limited;
mod links;
mod logs;
mod pods;
mod reason;
mod selector;
mod state;
mod statefulsets;
mod watch;
mod workload;

pub use cluster::{Backend, ClusterError, Connection, Contexts};
pub use daemonsets::{DaemonSetDetails, DaemonSetRow};
pub use deployments::{Condition, DeploymentDetails, DeploymentRow};
pub use details::{ContainerDetails, ObjectDetails, ObjectEvent, PodDetails};
pub use environment::use_login_shell_environment;
pub use follow::Follow;
pub use jobs::{JobDetails, JobRow};
pub use links::ObjectLink;
pub use logs::{
    Bucket, CONTEXT_SEPARATOR, ContextBlock, ContextLine, ExportProgress, FilterError,
    FilterProgress, FilterUpdates, Following, Gap, JsonLine, Level, LevelCounts, LogExport,
    LogFilter, LogFilterScan, LogLine, LogProgress, LogReader, LogTail, LogTarget, MAX_TAIL_PODS,
    Severity, TimelineView, export_directory, json_line,
};
pub use pods::{Owner, PodFeed, PodKey, PodRow, PodSnapshot, PodWatch, log_targets};
pub use selector::Selector;
pub use state::{SavedState, WindowRect, state_path};
pub use statefulsets::{ClaimTemplate, StatefulSetDetails, StatefulSetRow};
pub use watch::{Feed, ObjectKey, ResourceWatch, Row, Snapshot, WatchError};

pub const APP_NAME: &str = "Taffrail";
/// The bundle identifier, also used as the Windows app ID and the Linux window class.
pub const APP_ID: &str = "dev.opik.taffrail";
