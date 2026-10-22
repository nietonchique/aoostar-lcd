//! HDD temperature and self-test progress via `smartctl -j`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde_json::Value;

use crate::model::SelfTest;

use super::proc;

const SMARTCTL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartInfo {
    pub temp_c: Option<f32>,
    pub self_test: Option<SelfTest>,
    /// Overall health assessment explicitly FAILED (`smart_status.passed=false`).
    pub failed: bool,
}

pub fn read(by_id: &Path) -> anyhow::Result<SmartInfo> {
    let device = by_id.as_os_str();
    let args = [
        std::ffi::OsStr::new("-j"),
        std::ffi::OsStr::new("-H"),
        std::ffi::OsStr::new("-c"),
        std::ffi::OsStr::new("-A"),
        device,
    ];
    // smartctl exits with a bit mask (non-zero for e.g. old log errors) while
    // still printing complete JSON, so the exit status is not inspected.
    let output = proc::run("smartctl", &args, SMARTCTL_TIMEOUT).context("smartctl")?;
    parse_smartctl(&output.stdout)
}

pub fn parse_smartctl(json: &str) -> anyhow::Result<SmartInfo> {
    let root: Value = serde_json::from_str(json).context("smartctl json")?;
    let temp_c = root
        .pointer("/temperature/current")
        .and_then(Value::as_f64)
        .map(|temp| temp as f32);
    let status = root.pointer("/ata_smart_data/self_test/status");
    let self_test = status.and_then(parse_self_test);
    if temp_c.is_none() && status.is_none() {
        return Err(anyhow!("smartctl returned no data"));
    }
    let failed = root
        .pointer("/smart_status/passed")
        .and_then(Value::as_bool)
        == Some(false);
    Ok(SmartInfo {
        temp_c,
        self_test,
        failed,
    })
}

/// The high nibble of the ATA self-test status is 0xF while a test runs; the
/// low nibble then holds the remaining work in 10 % steps.
fn parse_self_test(status: &Value) -> Option<SelfTest> {
    let value = status.get("value").and_then(Value::as_u64)?;
    if value >> 4 != 0xF {
        return None;
    }
    let remaining = status
        .get("remaining_percent")
        .and_then(Value::as_u64)
        .or(Some((value & 0xF) * 10))
        .map(|percent| percent.min(100) as u8);
    Some(SelfTest {
        remaining_percent: remaining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: &str = r#"{"smartctl":{"exit_status":0},
"temperature":{"current":40},
"ata_smart_data":{"self_test":{"status":{"value":0,"string":"completed without error","passed":true},
"polling_minutes":{"short":2,"extended":1335}}}}"#;

    const RUNNING: &str = r#"{"smartctl":{"exit_status":64},
"temperature":{"current":43},
"ata_smart_data":{"self_test":{"status":{"value":249,"string":"self-test routine in progress","remaining_percent":90},
"polling_minutes":{"short":2,"extended":1335}}}}"#;

    #[test]
    fn idle_drive() {
        let info = parse_smartctl(IDLE).unwrap();
        assert_eq!(info.temp_c, Some(40.0));
        assert!(info.self_test.is_none());
        assert!(!info.failed);
    }

    #[test]
    fn failed_health_assessment() {
        let json = IDLE.replace(
            "\"temperature\"",
            "\"smart_status\":{\"passed\":false},\"temperature\"",
        );
        assert!(parse_smartctl(&json).unwrap().failed);
        let passed = IDLE.replace(
            "\"temperature\"",
            "\"smart_status\":{\"passed\":true},\"temperature\"",
        );
        assert!(!parse_smartctl(&passed).unwrap().failed);
    }

    #[test]
    fn drive_running_self_test() {
        let info = parse_smartctl(RUNNING).unwrap();
        assert_eq!(info.temp_c, Some(43.0));
        assert_eq!(
            info.self_test,
            Some(SelfTest {
                remaining_percent: Some(90)
            })
        );
    }

    #[test]
    fn remaining_falls_back_to_status_nibble() {
        let json = RUNNING.replace(",\"remaining_percent\":90", "");
        let info = parse_smartctl(&json).unwrap();
        assert_eq!(info.self_test.unwrap().remaining_percent, Some(90));
    }

    #[test]
    fn permission_denied_output_is_an_error() {
        let denied = r#"{"smartctl":{"messages":[{"string":"open failed: Permission denied","severity":"error"}],"exit_status":2}}"#;
        assert!(parse_smartctl(denied).is_err());
        assert!(parse_smartctl("").is_err());
    }
}
