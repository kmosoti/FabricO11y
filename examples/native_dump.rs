//! Development-only exact log-body replay for the registered native trial.
use fabric_o11y::spindle::runtime::Config;
use fabric_o11y::spindle::spool::Spool;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use prost::Message;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: native_dump <CONFIG_PATH>",
        )
    })?;
    let config = Config::load(path)?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    Spool::inspect(&config.spool, config.spool_bytes - 4096, |batch| {
        if batch.logs.is_empty() {
            return Ok(());
        }
        let request = ExportLogsServiceRequest::decode(batch.logs.as_slice()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid committed OTLP logs")
        })?;
        for resource in request.resource_logs {
            for scope in resource.scope_logs {
                for record in scope.log_records {
                    let body = record.body.and_then(|v| v.value).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing committed log body")
                    })?;
                    let any_value::Value::StringValue(body) = body else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unsupported log body",
                        ));
                    };
                    output.write_all(body.as_bytes())?;
                    output.write_all(b"\n")?;
                }
            }
        }
        Ok(())
    })?;
    output.flush()
}
