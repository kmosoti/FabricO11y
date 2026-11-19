//! Process-independent global run correlation, carried unchanged across processes.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunContext {
    pub run_id: String,
}
impl RunContext {
    pub fn new() -> Result<Self, String> {
        let context = Self {
            run_id: std::fs::read_to_string("/proc/sys/kernel/random/uuid")
                .map_err(|e| e.to_string())?
                .trim()
                .to_owned(),
        };
        context.validate()?;
        Ok(context)
    }
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.run_id).map_err(|()| "run_id must be a lowercase UUID".into())
    }
}

pub(crate) fn validate_id(id: &str) -> Result<(), ()> {
    let bytes = id.as_bytes();
    if id == "00000000-0000-0000-0000-000000000000"
        || bytes.len() != 36
        || !bytes.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
    {
        return Err(());
    }
    Ok(())
}
