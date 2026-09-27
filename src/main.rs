use fabric_o11y::Event;
use fabric_o11y::buffer::EventBuffer;
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::log::{EventLog, same_record_contents};
use std::io;
use std::num::NonZeroUsize;
use std::process::ExitCode;

fn print_batch(batch: Vec<Event>) {
    println!("batch of {} event(s):", batch.len());
    for event in batch {
        println!("{event:?}");
    }
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -- [<SEED:u64> <EVENTS:u32> | write <PATH> <SEED:u64> <EVENTS:u32> | replay <PATH>]"
    );
}

fn parse_workload(seed: &str, events: &str) -> Option<WorkloadConfig> {
    Some(WorkloadConfig {
        seed: seed.parse().ok()?,
        events: events.parse().ok()?,
    })
}

fn run_demo(config: WorkloadConfig) {
    let capacity = NonZeroUsize::new(2).unwrap();
    let batch_size = NonZeroUsize::new(2).unwrap();
    let mut buffer = EventBuffer::new(capacity);

    for event in EventGenerator::new(config) {
        if let Err(event) = buffer.try_push(event) {
            println!("buffer full: event {} returned to caller", event.id.0);
            print_batch(buffer.take_batch(batch_size));
            buffer
                .try_push(event)
                .expect("a nonempty batch made room for the returned event");
        }
    }

    while !buffer.is_empty() {
        print_batch(buffer.take_batch(batch_size));
    }
}

fn commit_batch(log: &mut EventLog, batch: Vec<Event>) -> io::Result<()> {
    for event in batch {
        // `append` borrows the event. The log owns a recoverable copy only
        // after it returns Ok; until then this local value remains ours.
        log.append(&event)?;
        println!("committed event {}", event.id.0);
    }
    Ok(())
}

fn run_write(path: &str, config: WorkloadConfig) -> io::Result<()> {
    let mut log = EventLog::open(path)?;
    let mut source = EventGenerator::new(config);

    // Repeating the same command reconstructs this synthetic upstream source.
    // A recovered log must be an exact prefix; otherwise appending would mix
    // different workloads and incorrectly treat an ID collision as a retry.
    let committed = log.replay(|stored| {
        let Some(expected) = source.next() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "log is longer than this seed/count workload",
            ));
        };
        if same_record_contents(&expected, &stored)? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "log is not a prefix of this seed/count workload",
            ))
        }
    })?;
    if committed > 0 {
        println!("already committed {committed} event(s)");
    }

    let capacity = NonZeroUsize::new(2).unwrap();
    let batch_size = NonZeroUsize::new(2).unwrap();
    let mut buffer = EventBuffer::new(capacity);
    for event in source {
        if let Err(event) = buffer.try_push(event) {
            commit_batch(&mut log, buffer.take_batch(batch_size))?;
            buffer
                .try_push(event)
                .expect("a nonempty batch made room for the returned event");
        }
    }
    while !buffer.is_empty() {
        commit_batch(&mut log, buffer.take_batch(batch_size))?;
    }
    Ok(())
}

fn run_replay(path: &str) -> io::Result<()> {
    let mut log = EventLog::open(path)?;
    log.replay(|event| {
        println!("{event:?}");
        Ok(())
    })?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let operation = match args.as_slice() {
        [] => {
            run_demo(WorkloadConfig {
                seed: 42,
                events: 3,
            });
            return ExitCode::SUCCESS;
        }
        [seed, events] if seed != "replay" => match parse_workload(seed, events) {
            Some(config) => {
                run_demo(config);
                return ExitCode::SUCCESS;
            }
            None => {
                print_usage();
                return ExitCode::from(2);
            }
        },
        [mode, path, seed, events] if mode == "write" => {
            parse_workload(seed, events).map(|config| run_write(path, config))
        }
        [mode, path] if mode == "replay" => Some(run_replay(path)),
        _ => None,
    };

    match operation {
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(error)) => {
            eprintln!("log operation failed: {error}");
            eprintln!(
                "if a storage write or sync failed, rebuild the log from an independent source on healthy storage before resuming"
            );
            ExitCode::FAILURE
        }
        None => {
            print_usage();
            ExitCode::from(2)
        }
    }
}
