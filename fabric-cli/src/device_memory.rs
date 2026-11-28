use fabric_collect::device_memory::{
    DeviceMemoryMax, DeviceMemoryProbeError, DeviceMemoryRegion, DeviceMemoryScope,
    DeviceMemorySnapshot, probe_device_memory_at,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Normalized cgroup v2 membership path to inspect (for example /user.slice/job.scope).
    #[arg(long, required = true)]
    pub(crate) cgroup: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Available,
    InterfaceUnavailable,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Scope {
    Root,
    NonRoot,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "bytes", rename_all = "snake_case")]
enum Max {
    Bytes(u64),
    Unbounded,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct Region {
    identifier: String,
    capacity_bytes: Option<u64>,
    current_bytes: Option<u64>,
    peak_bytes: Option<u64>,
    max_bytes: Option<Max>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct Report {
    availability: Availability,
    cgroup_path: String,
    scope: Option<Scope>,
    regions: Vec<Region>,
}

fn report(snapshot: Option<DeviceMemorySnapshot>, selected_cgroup: &str) -> Report {
    let Some(snapshot) = snapshot else {
        return Report {
            availability: Availability::InterfaceUnavailable,
            cgroup_path: selected_cgroup.to_owned(),
            scope: None,
            regions: Vec::new(),
        };
    };

    let scope = match snapshot.scope {
        DeviceMemoryScope::Root => Scope::Root,
        DeviceMemoryScope::NonRoot => Scope::NonRoot,
    };
    let mut regions: Vec<_> = snapshot.regions.into_iter().map(region).collect();
    regions.sort_by(|left, right| left.identifier.cmp(&right.identifier));
    Report {
        availability: Availability::Available,
        cgroup_path: snapshot.cgroup_path,
        scope: Some(scope),
        regions,
    }
}

fn region(value: DeviceMemoryRegion) -> Region {
    Region {
        identifier: value.identifier,
        capacity_bytes: value.capacity_bytes,
        current_bytes: value.current_bytes,
        peak_bytes: value.peak_bytes,
        max_bytes: value.max_bytes.map(|max| match max {
            DeviceMemoryMax::Bytes(bytes) => Max::Bytes(bytes),
            DeviceMemoryMax::Unbounded => Max::Unbounded,
        }),
    }
}

pub(crate) fn run(args: &Args, unsupported_format: bool) -> Result<(), String> {
    if unsupported_format {
        return Err("device-memory supports JSON output only".into());
    }
    let snapshot = probe_device_memory_at(&args.cgroup)
        .map_err(|error: DeviceMemoryProbeError| format!("device-memory probe: {error}"))?;
    super::emit_json("device-memory", &report(snapshot, &args.cgroup))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_preserves_missing_zero_and_explicit_unbounded_values() {
        let output = report(
            Some(DeviceMemorySnapshot {
                cgroup_path: "/user.slice/job.scope".into(),
                scope: DeviceMemoryScope::NonRoot,
                regions: vec![
                    DeviceMemoryRegion {
                        identifier: "z-device".into(),
                        capacity_bytes: None,
                        current_bytes: Some(0),
                        peak_bytes: None,
                        max_bytes: Some(DeviceMemoryMax::Unbounded),
                    },
                    DeviceMemoryRegion {
                        identifier: "a-device".into(),
                        capacity_bytes: None,
                        current_bytes: None,
                        peak_bytes: None,
                        max_bytes: Some(DeviceMemoryMax::Bytes(24)),
                    },
                ],
            }),
            "/ignored-on-success",
        );
        let json = serde_json::to_value(output).unwrap();
        assert_eq!(json["availability"], "available");
        assert_eq!(json["scope"], "non_root");
        assert_eq!(json["regions"][0]["identifier"], "a-device");
        assert_eq!(json["regions"][0]["max_bytes"]["kind"], "bytes");
        assert_eq!(json["regions"][0]["max_bytes"]["bytes"], 24);
        assert_eq!(json["regions"][1]["identifier"], "z-device");
        assert!(json["regions"][1]["capacity_bytes"].is_null());
        assert_eq!(json["regions"][1]["current_bytes"], 0);
        assert!(json["regions"][1]["peak_bytes"].is_null());
        assert_eq!(json["regions"][1]["max_bytes"]["kind"], "unbounded");
        assert!(json["regions"][1]["max_bytes"].get("bytes").is_none());
    }

    #[test]
    fn schema_sorts_regions_and_marks_unavailable_interface() {
        let output = report(
            Some(DeviceMemorySnapshot {
                cgroup_path: "/".into(),
                scope: DeviceMemoryScope::Root,
                regions: vec![
                    DeviceMemoryRegion {
                        identifier: "z".into(),
                        capacity_bytes: Some(0),
                        current_bytes: None,
                        peak_bytes: None,
                        max_bytes: None,
                    },
                    DeviceMemoryRegion {
                        identifier: "a".into(),
                        capacity_bytes: Some(12),
                        current_bytes: None,
                        peak_bytes: None,
                        max_bytes: None,
                    },
                ],
            }),
            "/ignored-on-success",
        );
        let json = serde_json::to_value(output).unwrap();
        assert_eq!(json["scope"], "root");
        assert_eq!(json["regions"][0]["identifier"], "a");
        assert_eq!(json["regions"][1]["identifier"], "z");
        assert_eq!(json["regions"][1]["capacity_bytes"], 0);

        let unavailable = serde_json::to_value(report(None, "/user.slice/job.scope")).unwrap();
        assert_eq!(unavailable["availability"], "interface_unavailable");
        assert_eq!(unavailable["cgroup_path"], "/user.slice/job.scope");
        assert!(unavailable["scope"].is_null());
        assert_eq!(unavailable["regions"], serde_json::json!([]));
    }
}
