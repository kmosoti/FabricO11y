//! Bounded async Linux host, cgroup v2, systemd, filesystem, GPU, and OTLP collectors.

pub mod cgroups;
pub mod channel;
pub mod daemon;
pub mod fs_watch;
pub mod host;
pub mod self_guard;
pub mod spindle_bridge;
mod spindle_range;
pub mod systemd;

pub use channel::BoundedTelemetryQueue;
pub use daemon::{
    CollectedState, DaemonConfig, DaemonRequest, DaemonResponse, collect_once,
    default_daemon_socket, default_ingest_socket, default_spindle_socket, query_daemon_socket,
    query_running_daemon, run_daemon, run_spindle_only_daemon,
};
pub use self_guard::SelfPreservationGuard;
pub use spindle_bridge::{
    IngestEnvelope, SpindleProjectionPoll, observation_to_telemetry, parse_ingest_line,
    poll_spindle_custody_records, poll_spindle_projection_records,
};
pub use spindle_range::SpindleProjectionRange;
