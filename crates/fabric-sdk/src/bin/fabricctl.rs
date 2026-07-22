use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use fabric_sdk::{Engine, SdkError};
use serde::Serialize;
use serde_json::json;

enum Command {
    Admit { input: String },
    Seal,
    Replay,
    Validate,
    Locate { record_id: String },
}

fn main() -> ExitCode {
    match run() {
        Ok(value) => match print_json(&value) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        },
        Err(error) => fail(&error),
    }
}

fn run() -> Result<serde_json::Value, SdkError> {
    let (root, command) = parse_args(env::args().skip(1).collect())?;
    let mut engine = Engine::open(root)?;
    match command {
        Command::Admit { input } => {
            let json = if input == "-" {
                let mut json = String::new();
                io::stdin()
                    .read_to_string(&mut json)
                    .map_err(|error| SdkError::Io(error.to_string()))?;
                json
            } else {
                fs::read_to_string(input).map_err(|error| SdkError::Io(error.to_string()))?
            };
            serde_json::to_value(engine.admit_json(&json)?)
                .map_err(|error| SdkError::JsonInvalid(error.to_string()))
        }
        Command::Seal => serde_json::to_value(engine.seal()?)
            .map_err(|error| SdkError::JsonInvalid(error.to_string())),
        Command::Replay => serde_json::from_str(&engine.replay_json()?)
            .map_err(|error| SdkError::JsonInvalid(error.to_string())),
        Command::Validate => serde_json::to_value(engine.validate()?)
            .map_err(|error| SdkError::JsonInvalid(error.to_string())),
        Command::Locate { record_id } => serde_json::to_value(engine.locate(&record_id)?)
            .map_err(|error| SdkError::JsonInvalid(error.to_string())),
    }
}

fn parse_args(arguments: Vec<String>) -> Result<(PathBuf, Command), SdkError> {
    let Some(root) = arguments.first() else {
        return Err(usage());
    };
    let Some(command) = arguments.get(1) else {
        return Err(usage());
    };
    let command = match command.as_str() {
        "admit" if arguments.len() == 3 => Command::Admit {
            input: arguments[2].clone(),
        },
        "seal" if arguments.len() == 2 => Command::Seal,
        "replay" if arguments.len() == 2 => Command::Replay,
        "validate" if arguments.len() == 2 => Command::Validate,
        "locate" if arguments.len() == 3 => Command::Locate {
            record_id: arguments[2].clone(),
        },
        _ => return Err(usage()),
    };
    Ok((PathBuf::from(root), command))
}

fn usage() -> SdkError {
    SdkError::AdmissionInvalid(
        "usage: fabricctl ROOT admit FILE|- | seal | replay | validate | locate RECORD_ID"
            .to_owned(),
    )
}

fn print_json(value: &impl Serialize) -> Result<(), SdkError> {
    let output = serde_json::to_string_pretty(value)
        .map_err(|error| SdkError::JsonInvalid(error.to_string()))?;
    println!("{output}");
    Ok(())
}

fn fail(error: &SdkError) -> ExitCode {
    let value = json!({
        "category": error.category(),
        "message": error.to_string(),
    });
    eprintln!(
        "{}",
        serde_json::to_string(&value).unwrap_or_else(|_| {
            "{\"category\":\"json_invalid\",\"message\":\"failed to serialize error\"}".to_owned()
        })
    );
    ExitCode::FAILURE
}
