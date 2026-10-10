//! Diagnostic replay with native commit-group labels, for snapshot oracle input.
//! Run only after stopping the writer; its native lock rejects a live owner.
use fabric_frame::frame::FrameLog;
use fabric_server::{config::Config, segment, store::Group};
use prost::Message;
use serde_json::json;
use std::io::{self, Write};
fn main() -> io::Result<()> {
    let config = Config::load(std::env::args().nth(1).expect("server config"))?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut emit = |group, entry: &fabric_server::store::Entry| -> io::Result<()> {
        // Hex encoding is outside measured native operation spans.
        writeln!(
            out,
            "{}",
            json!({"group":group,"label":entry.label,"received_ns":entry.received_unix_nano,
            "hex":entry.batch.iter().map(|b|format!("{b:02x}")).collect::<String>()})
        )
    };
    let mut covered = 0;
    for (label, manifest) in segment::list(&config.state_dir)? {
        let dir = config
            .state_dir
            .join("segments")
            .join(segment::segment_name(label));
        let mut result = Ok(());
        segment::scan_batches(&dir, &manifest, |group, entry| {
            if result.is_ok() {
                result = emit(group, &entry);
            }
        })?;
        result?;
        covered = manifest.last_group;
    }
    let log = FrameLog::open(
        &config.state_dir.join("journal"),
        config.journal_bytes,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )?;
    let mut pos = log.start_pos();
    while let Some((payload, next)) = log.read_at(pos)? {
        let group = Group::decode(payload.as_slice()).map_err(io::Error::other)?;
        if group.group_sequence > covered {
            for entry in &group.entries {
                emit(group.group_sequence, entry)?;
            }
        }
        pos = next;
    }
    out.flush()
}
