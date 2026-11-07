//! Cluster state, watches and log storage. This crate never imports GPUI.

mod cluster;

pub use cluster::{Backend, ClusterError, Connection, Contexts};

pub const APP_NAME: &str = "Taffrail";
