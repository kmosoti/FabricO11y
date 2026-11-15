//! Bounded async Linux host, cgroup v2, systemd, filesystem, GPU, and OTLP collectors.

pub mod cgroups;
pub mod channel;
pub mod daemon;
pub mod fs_watch;
pub mod host;
pub mod self_guard;
pub mod spindle_bridge;
pub mod systemd;

pub use channel::BoundedTelemetryQueue;
pub use daemon::{
    CollectedState, DaemonConfig, DaemonRequest, DaemonResponse, collect_once,
    default_daemon_socket, default_ingest_socket, query_running_daemon, run_daemon,
};
pub use self_guard::SelfPreservationGuard;
