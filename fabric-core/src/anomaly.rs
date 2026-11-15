//! `EwmaMadDetector` anomaly scoring and Five Data Observability Pillars evaluation.

use crate::model::{HostSnapshot, SCHEMA_VERSION};
use crate::sketch::DDSketch;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// Five Data Observability Pillars evaluated cleanly over telemetry state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PillarKind {
    Freshness,
    Volume,
    Schema,
    Distribution,
    Lineage,
}

impl fmt::Display for PillarKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Freshness => f.write_str("freshness"),
            Self::Volume => f.write_str("volume"),
            Self::Schema => f.write_str("schema"),
            Self::Distribution => f.write_str("distribution"),
            Self::Lineage => f.write_str("lineage"),
        }
    }
}

impl FromStr for PillarKind {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "freshness" => Ok(Self::Freshness),
            "volume" => Ok(Self::Volume),
            "schema" => Ok(Self::Schema),
            "distribution" => Ok(Self::Distribution),
            "lineage" => Ok(Self::Lineage),
            other => Err(format!(
                "unknown pillar '{other}' (expected freshness, volume, schema, distribution, or lineage)"
            )),
        }
    }
}

/// Severity classification emitted by pillar checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => f.write_str("info"),
            Self::Warn => f.write_str("warn"),
            Self::Critical => f.write_str("critical"),
        }
    }
}

/// Output of `EwmaMadDetector::observe`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AnomalyScore {
    pub value: f64,
    pub baseline: f64,
    pub mad: f64,
    pub z_score: f64,
    pub is_anomalous: bool,
}

/// Streaming EWMA + MAD (Median Absolute Deviation) anomaly scorer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EwmaMadDetector {
    alpha: f64,
    threshold_z: f64,
    warmup_samples: u64,
    samples: u64,
    ewma: f64,
    ewma_abs_dev: f64,
}

impl Default for EwmaMadDetector {
    fn default() -> Self {
        Self::new(0.25, 3.5, 6)
    }
}

impl EwmaMadDetector {
    /// Constructs a bounded streaming EWMA/MAD detector.
    #[must_use]
    pub fn new(alpha: f64, threshold_z: f64, warmup_samples: u64) -> Self {
        Self {
            alpha: alpha.clamp(0.01, 0.9),
            threshold_z: threshold_z.max(1.0),
            warmup_samples: warmup_samples.max(2),
            samples: 0,
            ewma: 0.0,
            ewma_abs_dev: 0.0,
        }
    }

    /// Incorporates a finite observation and scores whether it deviates anomalously.
    pub fn observe(&mut self, value: f64) -> AnomalyScore {
        if !value.is_finite() {
            return AnomalyScore {
                value,
                baseline: self.ewma,
                mad: self.ewma_abs_dev,
                z_score: 0.0,
                is_anomalous: false,
            };
        }
        if self.samples == 0 {
            self.samples = 1;
            self.ewma = value;
            self.ewma_abs_dev = 0.0;
            return AnomalyScore {
                value,
                baseline: value,
                mad: 0.0,
                z_score: 0.0,
                is_anomalous: false,
            };
        }

        let dev = (value - self.ewma).abs();
        let scale = (self.ewma_abs_dev * 1.4826)
            .max(self.ewma.abs() * 0.05)
            .max(1e-6);
        let z_score = dev / scale;
        let is_anomalous = self.samples >= self.warmup_samples && z_score >= self.threshold_z;

        self.samples = self.samples.saturating_add(1);
        // Update slowly on anomalies so spikes do not immediately poison baseline.
        let eff_alpha = if is_anomalous {
            self.alpha * 0.25
        } else {
            self.alpha
        };
        self.ewma = eff_alpha * value + (1.0 - eff_alpha) * self.ewma;
        self.ewma_abs_dev = eff_alpha * dev + (1.0 - eff_alpha) * self.ewma_abs_dev;

        AnomalyScore {
            value,
            baseline: self.ewma,
            mad: self.ewma_abs_dev,
            z_score,
            is_anomalous,
        }
    }

    #[must_use]
    pub const fn samples(&self) -> u64 {
        self.samples
    }
}

/// Specific drift or policy violation identified by a pillar evaluator.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PillarFinding {
    pub pillar: String,
    pub severity: String,
    pub target: String,
    pub summary: String,
    pub metric_value: f64,
    pub threshold_value: f64,
}

/// Summary report emitted by `fabric drift`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DriftReport {
    pub schema_version: String,
    pub generated_at_unix_ms: i64,
    pub total_findings: usize,
    pub critical_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
    pub findings: Vec<PillarFinding>,
}

/// Deterministic Sans-I/O context supplied to the Five Pillar evaluator.
#[derive(Debug, Clone, Default)]
pub struct DriftContext {
    pub now_unix_ms: i64,
    pub last_collection_watermark_ms: i64,
    pub max_staleness_ms: i64,
    pub observed_schemas: Vec<String>,
    pub expected_schemas: Vec<String>,
    pub volume_delta_bytes: f64,
    pub volume_score: Option<AnomalyScore>,
    pub latency_sketch: Option<DDSketch>,
}

/// Evaluates Freshness, Volume, Schema, Distribution, and Lineage pillars without I/O.
#[must_use]
pub fn evaluate_five_pillars(
    snapshot: &HostSnapshot,
    ctx: &DriftContext,
    filter: Option<PillarKind>,
) -> DriftReport {
    let mut findings = Vec::new();
    let include = |p: PillarKind| filter.is_none_or(|f| f == p);

    if include(PillarKind::Freshness) {
        let staleness_ms = (ctx.now_unix_ms - ctx.last_collection_watermark_ms).max(0);
        let max_stale = ctx.max_staleness_ms.max(15_000);
        if ctx.last_collection_watermark_ms == 0 || staleness_ms > max_stale {
            let severity = if staleness_ms > max_stale * 2 {
                Severity::Critical
            } else {
                Severity::Warn
            };
            findings.push(PillarFinding {
                pillar: PillarKind::Freshness.to_string(),
                severity: severity.to_string(),
                target: "collector:daemon".into(),
                summary: format!(
                    "collector watermark stale by {}ms (budget {}ms)",
                    staleness_ms, max_stale
                ),
                metric_value: staleness_ms as f64,
                threshold_value: max_stale as f64,
            });
        }
    }

    if include(PillarKind::Volume) {
        if let Some(score) = ctx.volume_score
            && score.is_anomalous
        {
            findings.push(PillarFinding {
                pillar: PillarKind::Volume.to_string(),
                severity: if score.z_score >= 6.0 {
                    Severity::Critical.to_string()
                } else {
                    Severity::Warn.to_string()
                },
                target: "fs:write_velocity".into(),
                summary: format!(
                    "anomalous storage volume delta {:.1} bytes (baseline {:.1}, z={:.2})",
                    score.value, score.baseline, score.z_score
                ),
                metric_value: score.z_score,
                threshold_value: 3.5,
            });
        }
        if snapshot.self_health.dropped_events_total > 0 {
            findings.push(PillarFinding {
                pillar: PillarKind::Volume.to_string(),
                severity: Severity::Warn.to_string(),
                target: "collector:channel_backpressure".into(),
                summary: format!(
                    "bounded collector queue shed {} events via try_send backpressure",
                    snapshot.self_health.dropped_events_total
                ),
                metric_value: snapshot.self_health.dropped_events_total as f64,
                threshold_value: 0.0,
            });
        }
        for mount in &snapshot.mounts {
            if mount.used_pct >= 85.0 {
                findings.push(PillarFinding {
                    pillar: PillarKind::Volume.to_string(),
                    severity: if mount.used_pct >= 95.0 {
                        Severity::Critical.to_string()
                    } else {
                        Severity::Warn.to_string()
                    },
                    target: format!("mount:{}", mount.mount_point),
                    summary: format!(
                        "mount {} at {:.1}% capacity ({} MiB used)",
                        mount.mount_point,
                        mount.used_pct,
                        mount.used_bytes / (1024 * 1024)
                    ),
                    metric_value: mount.used_pct,
                    threshold_value: 85.0,
                });
            }
        }
        for entry in &snapshot.fs_churn {
            if entry.size_bytes >= 128 * 1024 * 1024 && entry.recent_delta_bytes > 0 {
                findings.push(PillarFinding {
                    pillar: PillarKind::Volume.to_string(),
                    severity: if entry.mount_point == "/tmp"
                        || entry.size_bytes >= 1024 * 1024 * 1024
                    {
                        Severity::Critical.to_string()
                    } else {
                        Severity::Warn.to_string()
                    },
                    target: format!("path:{}", entry.path),
                    summary: format!(
                        "high-churn path {} grew to {} MiB on {}",
                        entry.path,
                        entry.size_bytes / (1024 * 1024),
                        entry.mount_point
                    ),
                    metric_value: (entry.size_bytes / (1024 * 1024)) as f64,
                    threshold_value: 128.0,
                });
            }
        }
    }

    if include(PillarKind::Schema) {
        let expected: BTreeSet<&str> = if ctx.expected_schemas.is_empty() {
            [SCHEMA_VERSION].into_iter().collect()
        } else {
            ctx.expected_schemas.iter().map(String::as_str).collect()
        };
        for schema in &ctx.observed_schemas {
            if !expected.contains(schema.as_str()) {
                findings.push(PillarFinding {
                    pillar: PillarKind::Schema.to_string(),
                    severity: Severity::Warn.to_string(),
                    target: format!("schema:{schema}"),
                    summary: format!("unexpected telemetry schema '{schema}' observed"),
                    metric_value: 1.0,
                    threshold_value: 0.0,
                });
            }
        }
    }

    if include(PillarKind::Distribution) {
        if let Some(sketch) = &ctx.latency_sketch
            && let (Some(p50), Some(p99)) = (sketch.quantile(0.50), sketch.quantile(0.99))
            && p50 > 0.0
            && (p99 / p50) >= 8.0
        {
            findings.push(PillarFinding {
                pillar: PillarKind::Distribution.to_string(),
                severity: Severity::Warn.to_string(),
                target: "sketch:tail_latency".into(),
                summary: format!(
                    "heavy-tailed latency distribution p99={p99:.2}ms vs p50={p50:.2}ms"
                ),
                metric_value: p99 / p50,
                threshold_value: 8.0,
            });
        }
        if snapshot.psi.memory_some.avg10 >= 5.0 || snapshot.psi.io_some.avg10 >= 10.0 {
            findings.push(PillarFinding {
                pillar: PillarKind::Distribution.to_string(),
                severity: Severity::Warn.to_string(),
                target: "kernel:psi".into(),
                summary: format!(
                    "PSI stall elevated mem.some.avg10={:.2} io.some.avg10={:.2}",
                    snapshot.psi.memory_some.avg10, snapshot.psi.io_some.avg10
                ),
                metric_value: snapshot
                    .psi
                    .memory_some
                    .avg10
                    .max(snapshot.psi.io_some.avg10),
                threshold_value: 5.0,
            });
        }
        if snapshot.memory.swap_total_bytes > 0 {
            let swap_pct = (snapshot.memory.swap_used_bytes as f64
                / snapshot.memory.swap_total_bytes as f64)
                * 100.0;
            if swap_pct >= 75.0 {
                findings.push(PillarFinding {
                    pillar: PillarKind::Distribution.to_string(),
                    severity: if swap_pct >= 90.0 {
                        Severity::Critical.to_string()
                    } else {
                        Severity::Warn.to_string()
                    },
                    target: "kernel:zram0".into(),
                    summary: format!("swap/zram saturation at {swap_pct:.1}%"),
                    metric_value: swap_pct,
                    threshold_value: 75.0,
                });
            }
        }
        for slice in &snapshot.slices {
            if let Some(high) = slice.memory_high_bytes
                && high > 0
            {
                let ratio = (slice.memory_current_bytes as f64) / (high as f64);
                if ratio >= 0.80 {
                    findings.push(PillarFinding {
                        pillar: PillarKind::Distribution.to_string(),
                        severity: if ratio >= 0.95 {
                            Severity::Critical.to_string()
                        } else {
                            Severity::Warn.to_string()
                        },
                        target: format!("slice:{}", slice.slice_name),
                        summary: format!(
                            "{} at {:.1}% of MemoryHigh ({} MiB / {} MiB)",
                            slice.slice_name,
                            ratio * 100.0,
                            slice.memory_current_bytes / (1024 * 1024),
                            high / (1024 * 1024)
                        ),
                        metric_value: ratio * 100.0,
                        threshold_value: 80.0,
                    });
                }
            }
        }
    }

    if include(PillarKind::Lineage) {
        for unit in &snapshot.units {
            if let Some(reason) = &unit.orphaned_reason {
                findings.push(PillarFinding {
                    pillar: PillarKind::Lineage.to_string(),
                    severity: if unit.active_state == "failed" {
                        Severity::Critical.to_string()
                    } else {
                        Severity::Warn.to_string()
                    },
                    target: format!("unit:{}", unit.name),
                    summary: format!("unit {} in {}: {reason}", unit.name, unit.slice),
                    metric_value: 1.0,
                    threshold_value: 0.0,
                });
            }
        }
        for entry in &snapshot.fs_churn {
            if entry.size_bytes >= 64 * 1024 * 1024 && entry.owner_pid.is_none() {
                findings.push(PillarFinding {
                    pillar: PillarKind::Lineage.to_string(),
                    severity: Severity::Warn.to_string(),
                    target: format!("path:{}", entry.path),
                    summary: format!(
                        "unattributed {} MiB high-churn artifact at {} has no live owning PID",
                        entry.size_bytes / (1024 * 1024),
                        entry.path
                    ),
                    metric_value: (entry.size_bytes / (1024 * 1024)) as f64,
                    threshold_value: 64.0,
                });
            }
        }
    }

    let mut critical_count = 0;
    let mut warn_count = 0;
    let mut info_count = 0;
    for f in &findings {
        match f.severity.as_str() {
            "critical" => critical_count += 1,
            "warn" => warn_count += 1,
            _ => info_count += 1,
        }
    }

    DriftReport {
        schema_version: SCHEMA_VERSION.to_owned(),
        generated_at_unix_ms: snapshot.timestamp_unix_ms,
        total_findings: findings.len(),
        critical_count,
        warn_count,
        info_count,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FsChurnEntry, SelfHealthSummary, UnitRecord};

    #[test]
    fn ewma_mad_detects_spike_after_warmup() {
        let mut detector = EwmaMadDetector::new(0.3, 3.5, 5);
        for v in [100.0, 102.0, 99.0, 101.0, 100.0] {
            assert!(!detector.observe(v).is_anomalous);
        }
        let spike = detector.observe(500.0);
        assert!(spike.is_anomalous);
        assert!(spike.z_score >= 3.5);
    }

    #[test]
    fn five_pillars_flag_missing_collect_and_unattributed_churn() {
        let snap = HostSnapshot {
            timestamp_unix_ms: 100_000,
            units: vec![UnitRecord {
                name: "metron-orphan.scope".into(),
                manager: "user".into(),
                slice: "agent.slice".into(),
                active_state: "failed".into(),
                sub_state: "failed".into(),
                transient: true,
                collect_mode_enabled: false,
                orphaned_reason: Some(
                    "transient unit missing CollectMode=inactive-or-failed".into(),
                ),
                ..Default::default()
            }],
            fs_churn: vec![FsChurnEntry {
                path: "/data/tmp/stale-blob".into(),
                mount_point: "/data".into(),
                size_bytes: 256 * 1024 * 1024,
                recent_delta_bytes: 256 * 1024 * 1024,
                ..Default::default()
            }],
            self_health: SelfHealthSummary::default(),
            ..Default::default()
        };
        let ctx = DriftContext {
            now_unix_ms: 150_000,
            last_collection_watermark_ms: 100_000,
            max_staleness_ms: 10_000,
            observed_schemas: vec![SCHEMA_VERSION.into(), "rogue.v0".into()],
            expected_schemas: vec![SCHEMA_VERSION.into()],
            ..Default::default()
        };
        let report = evaluate_five_pillars(&snap, &ctx, None);
        assert!(report.findings.iter().any(|f| f.pillar == "freshness"));
        assert!(report.findings.iter().any(|f| f.pillar == "schema"));
        assert!(report.findings.iter().any(|f| f.pillar == "volume"));
        assert!(report.findings.iter().any(|f| f.pillar == "lineage"));
    }
}
