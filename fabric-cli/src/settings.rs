use super::{Cli, DaemonArgs, fabrico11y_config_dir};
use clap::{ArgMatches, parser::ValueSource};
use fabric::{config, resources};
use fabric_collect::{
    DaemonConfig,
    resource_budget::{RingBudget, automatic_ring_budget},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    #[serde(default)]
    daemon: CollectorFields,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CollectorFields {
    data_dir: Option<PathBuf>,
    tmp_dir: Option<PathBuf>,
    interval_ms: Option<u64>,
    max_ticks: Option<u64>,
    spindle_socket: Option<PathBuf>,
    ring_budget_bytes: Option<usize>,
    max_disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Source {
    CommandLine,
    Environment,
    Profile,
    Default,
    Automatic,
    Missing,
}

pub(super) struct Effective {
    pub config: DaemonConfig,
    automatic_budget: RingBudget,
    sources: BTreeMap<&'static str, Source>,
}

#[derive(Serialize)]
struct Report<'a> {
    version: i64,
    profile: Option<&'a Path>,
    profile_status: Source,
    daemon: CollectorFields,
    daemon_mode: &'static str,
    ring_budget: AutomaticRingReport,
    sources: &'a BTreeMap<&'static str, Source>,
    spindle: ServiceHint<super::SpindleFileConfigHint>,
    server: ServiceHint<super::ServerFileConfigHint>,
    receiver_resources: Option<ReceiverResourcePreview>,
}

#[derive(Serialize)]
struct ReceiverResourcePreview {
    scope: &'static str,
    #[serde(flatten)]
    resources: resources::ReceiverResources,
}

#[derive(Serialize)]
struct AutomaticRingReport {
    automatic_bytes: usize,
    cgroup_limit_bytes: Option<u64>,
}

#[derive(Serialize)]
struct ServiceHint<T> {
    status: &'static str,
    path: Option<PathBuf>,
    fields: Option<T>,
}

pub(super) fn load_profile(explicit: Option<&Path>) -> Result<(Option<PathBuf>, Profile), String> {
    if let Some(path) = explicit {
        if path.extension().is_none_or(|extension| extension != "toml") {
            return Err("the Fabric settings profile must use a .toml filename".into());
        }
        return Ok((Some(path.to_owned()), config::load(path)?));
    }
    if let Some(dir) = fabrico11y_config_dir() {
        let path = dir.join("fabric.toml");
        if let Some(profile) = config::load_optional(&path)? {
            return Ok((Some(path), profile));
        }
    }
    Ok((None, Profile::default()))
}

fn arg_source(matches: &ArgMatches, id: &str) -> Source {
    match matches.value_source(id) {
        Some(ValueSource::EnvVariable) => Source::Environment,
        _ => Source::CommandLine,
    }
}

fn selected<T>(
    argument: Option<T>,
    argument_source: Source,
    profile: Option<T>,
    fallback: T,
) -> (T, Source) {
    match (argument, profile) {
        (Some(value), _) => (value, argument_source),
        (None, Some(value)) => (value, Source::Profile),
        (None, None) => (fallback, Source::Default),
    }
}

pub(super) fn data_dir(cli: &Cli, matches: &ArgMatches, profile: &Profile) -> (PathBuf, Source) {
    selected(
        cli.data_dir.clone(),
        arg_source(matches, "data_dir"),
        profile.daemon.data_dir.clone(),
        PathBuf::from(fabric_core::DEFAULT_DATA_DIR),
    )
}

pub(super) fn tmp_dir(
    cli: &Cli,
    matches: &ArgMatches,
    profile: &Profile,
) -> Result<(PathBuf, Source), String> {
    if let Some(path) = cli.tmp_dir.as_deref() {
        return Ok((
            super::resolve_cli_tmp_dir(Some(path))?,
            arg_source(matches, "tmp_dir"),
        ));
    }
    if let Some(path) = profile.daemon.tmp_dir.as_deref() {
        return Ok((super::resolve_cli_tmp_dir(Some(path))?, Source::Profile));
    }
    if let Some(path) = std::env::var_os("TMPDIR").filter(|path| !path.is_empty())
        && let Ok(path) = fabric_store::validate_non_tmp_path_on_disk(Path::new(&path))
    {
        return Ok((path, Source::Environment));
    }
    Ok((
        super::resolve_cli_tmp_dir(Some(Path::new(fabric_core::DEFAULT_TMP_DIR)))?,
        Source::Default,
    ))
}

pub(super) fn effective(
    cli: &Cli,
    matches: &ArgMatches,
    profile: &Profile,
    args: &DaemonArgs,
) -> Result<Effective, String> {
    let Some((_, command)) = matches.subcommand() else {
        return Err("missing daemon arguments".into());
    };
    let automatic_budget = automatic_ring_budget();
    let mut config = DaemonConfig::with_ring_budget(automatic_budget.budget_bytes);
    let mut sources = BTreeMap::new();
    let (resolved_data, data_source) = data_dir(cli, matches, profile);
    config.data_dir = resolved_data;
    sources.insert("data_dir", data_source);
    let (resolved_tmp, tmp_source) = tmp_dir(cli, matches, profile)?;
    config.tmp_dir = resolved_tmp;
    sources.insert("tmp_dir", tmp_source);
    let daemon = &profile.daemon;

    let (value, source) = selected(
        args.interval_ms,
        Source::CommandLine,
        daemon.interval_ms,
        config.interval_ms,
    );
    config.interval_ms = value.max(250);
    sources.insert("interval_ms", source);
    let (value, source) = selected(
        args.max_ticks.map(Some),
        Source::CommandLine,
        daemon.max_ticks.map(Some),
        config.max_ticks,
    );
    config.max_ticks = value;
    sources.insert(
        "max_ticks",
        if value.is_none() {
            Source::Missing
        } else {
            source
        },
    );
    let (value, source) = selected(
        super::non_empty_path(args.spindle_socket.clone()).map(Some),
        arg_source(command, "spindle_socket"),
        super::non_empty_path(daemon.spindle_socket.clone()).map(Some),
        config.spindle_socket.take(),
    );
    config.spindle_socket = value
        .map(|path| {
            fabric_store::validate_non_tmp_path_on_disk(&path).map_err(|err| err.to_string())
        })
        .transpose()?;
    sources.insert(
        "spindle_socket",
        if config.spindle_socket.is_none() {
            Source::Missing
        } else {
            source
        },
    );
    if args.spindle_only && matches!(sources["spindle_socket"], Source::Missing | Source::Default) {
        return Err("Spindle-only projection requires a configured Spindle socket".into());
    }
    let (value, source) = selected(
        args.ring_budget_bytes,
        Source::CommandLine,
        daemon.ring_budget_bytes,
        config.ring_budget_bytes,
    );
    const MIN_RING_BYTES: usize = 4096;
    let maximum = fabric_store::DEFAULT_RING_BUDGET_BYTES;
    if !(MIN_RING_BYTES..=maximum).contains(&value) {
        return Err(format!(
            "ring_budget_bytes must be in {MIN_RING_BYTES}..={maximum}"
        ));
    }
    config.ring_budget_bytes = value;
    sources.insert(
        "ring_budget_bytes",
        if source == Source::Default {
            Source::Automatic
        } else {
            source
        },
    );
    let (value, source) = selected(
        args.max_disk_bytes,
        Source::CommandLine,
        daemon.max_disk_bytes,
        config.max_disk_bytes,
    );
    if value == 0 || value > i64::MAX as u64 {
        return Err("max_disk_bytes must be positive and fit a TOML integer".into());
    }
    config.max_disk_bytes = value;
    sources.insert("max_disk_bytes", source);
    Ok(Effective {
        config,
        automatic_budget,
        sources,
    })
}

fn service_hint<T: serde::de::DeserializeOwned>(stem: &str) -> Result<ServiceHint<T>, String> {
    if let Some(dir) = fabrico11y_config_dir()
        && let Some((path, fields)) = config::discover(&dir, stem)?
    {
        return Ok(ServiceHint {
            status: "found",
            path: Some(path),
            fields: Some(fields),
        });
    }
    Ok(ServiceHint {
        status: "missing",
        path: None,
        fields: None,
    })
}

pub(super) fn print(
    cli: &Cli,
    args: &DaemonArgs,
    mut settings: Effective,
    profile: Option<&Path>,
) -> Result<(), String> {
    if cli.arrow || cli.table {
        return Err("fabric settings supports TOML and --json output".into());
    }
    let config = &mut settings.config;
    config.data_dir = fabric_store::validated_storage_layout(&config.data_dir)?
        .data_dir()
        .to_path_buf();
    let mut spindle: ServiceHint<super::SpindleFileConfigHint> = service_hint("spindle")?;
    if let Some(forwarding) = spindle
        .fields
        .as_mut()
        .and_then(|fields| fields.forwarding.as_mut())
    {
        forwarding.endpoint = listener_address(forwarding.endpoint.take());
    }
    let mut server: ServiceHint<super::ServerFileConfigHint> = service_hint("server")?;
    if let Some(fields) = server.fields.as_mut() {
        fields.listen = listener_address(fields.listen.take());
    }
    let receiver_resources = server.fields.as_ref().and_then(|fields| {
        let configured = fields
            .max_connections
            .unwrap_or(resources::DEFAULT_RECEIVER_CONNECTIONS);
        if !(1..=resources::MAX_RECEIVER_CONNECTIONS).contains(&configured) {
            server.status = "invalid_connection_limit";
            return None;
        }
        Some(ReceiverResourcePreview {
            scope: "settings_process",
            resources: resources::receiver_resources(configured),
        })
    });
    let report = Report {
        version: config::VERSION,
        profile,
        profile_status: if profile.is_some() {
            Source::Profile
        } else {
            Source::Missing
        },
        daemon: CollectorFields {
            data_dir: Some(config.data_dir.clone()),
            tmp_dir: Some(config.tmp_dir.clone()),
            interval_ms: Some(config.interval_ms),
            max_ticks: config.max_ticks,
            spindle_socket: config.spindle_socket.clone(),
            ring_budget_bytes: Some(config.ring_budget_bytes),
            max_disk_bytes: Some(config.max_disk_bytes),
        },
        daemon_mode: match (args.once, args.spindle_only) {
            (true, _) => "once",
            (_, true) => "spindle_only",
            _ => "resident",
        },
        ring_budget: AutomaticRingReport {
            automatic_bytes: settings.automatic_budget.budget_bytes,
            cgroup_limit_bytes: settings.automatic_budget.cgroup_limit_bytes,
        },
        sources: &settings.sources,
        spindle,
        server,
        receiver_resources,
    };
    if cli.json {
        super::emit_json("settings", &report)
    } else {
        print!(
            "{}",
            toml::to_string_pretty(&report).map_err(|err| err.to_string())?
        );
        Ok(())
    }
}

fn listener_address(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .and_then(|raw| raw.parse::<std::net::SocketAddr>().ok())
        .filter(|address| address.ip().is_loopback())
        .map(|address| address.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_never_render_url_credentials_as_listener_addresses() {
        assert_eq!(
            listener_address(Some("127.0.0.1:17440".into())).as_deref(),
            Some("127.0.0.1:17440")
        );
        assert_eq!(
            listener_address(Some("http://user:secret@127.0.0.1:17440".into())),
            None
        );
    }

    #[test]
    fn example_is_a_valid_profile() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/fabric.toml");
        let (_, profile) = load_profile(Some(&path)).unwrap();
        assert_eq!(profile.daemon.interval_ms, Some(2_000));
        assert!(profile.daemon.ring_budget_bytes.is_none());
    }
}
