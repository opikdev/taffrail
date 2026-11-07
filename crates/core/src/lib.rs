//! Cluster state, watches and log storage. This crate never imports GPUI.

mod cluster;
mod pods;

pub use cluster::{Backend, ClusterError, Connection, Contexts};
pub use pods::{PodRow, PodSnapshot, PodWatch};

pub const APP_NAME: &str = "Taffrail";
