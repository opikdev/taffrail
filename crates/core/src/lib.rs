//! Cluster state, watches and log storage. This crate never imports GPUI.

mod cluster;
mod logs;
mod pods;

pub use cluster::{Backend, ClusterError, Connection, Contexts};
pub use logs::{LogLine, LogProgress, LogReader, LogTail, LogTarget, MAX_TAIL_PODS};
pub use pods::{PodRow, PodSnapshot, PodWatch};

pub const APP_NAME: &str = "Taffrail";
