//! Woodpecker queue depth and oldest queued task age from Prometheus.

use anyhow::{Context, anyhow};
use serde_json::Value;

const QUEUED_QUERY: &str = "sum(homelab_woodpecker_queued_tasks)";
const OLDEST_QUERY: &str = "max(homelab_woodpecker_oldest_queued_task_age_seconds)";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Queue {
    pub queued: Option<u32>,
    pub oldest_secs: Option<u64>,
}

pub fn fetch(base_url: &str) -> anyhow::Result<Queue> {
    let queued = query(base_url, QUEUED_QUERY)?;
    let oldest = query(base_url, OLDEST_QUERY)?;
    Ok(Queue {
        // An empty `sum` means nothing is queued.
        queued: Some(to_count(queued.unwrap_or(0.0))),
        oldest_secs: oldest.map(|secs| secs.max(0.0).round() as u64),
    })
}

fn to_count(value: f64) -> u32 {
    value.max(0.0).round().min(f64::from(u32::MAX)) as u32
}

fn query(base_url: &str, expression: &str) -> anyhow::Result<Option<f64>> {
    let url = format!("{}/api/v1/query", base_url.trim_end_matches('/'));
    let body = super::http_agent()
        .get(&url)
        .query("query", expression)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, _) => anyhow!("prometheus HTTP {code}"),
            ureq::Error::Transport(transport) => anyhow!("prometheus {}", transport.kind()),
        })?
        .into_string()
        .context("reading prometheus response")?;
    parse_scalar(&body)
}

/// First sample of an instant-vector response; `None` for an empty vector.
pub fn parse_scalar(json: &str) -> anyhow::Result<Option<f64>> {
    let root: Value = serde_json::from_str(json).context("prometheus json")?;
    if root.get("status").and_then(Value::as_str) != Some("success") {
        return Err(anyhow!("prometheus query failed"));
    }
    let Some(first) = root
        .pointer("/data/result")
        .and_then(Value::as_array)
        .and_then(|samples| samples.first())
    else {
        return Ok(None);
    };
    let value = first
        .pointer("/value/1")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow!("prometheus sample has no numeric value"))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_with_value() {
        let json = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1792398600.123,"3"]}]}}"#;
        assert_eq!(parse_scalar(json).unwrap(), Some(3.0));
        let fractional =
            r#"{"status":"success","data":{"result":[{"metric":{},"value":[1,"412.5"]}]}}"#;
        assert_eq!(parse_scalar(fractional).unwrap(), Some(412.5));
    }

    #[test]
    fn empty_vector_is_none() {
        let json = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;
        assert_eq!(parse_scalar(json).unwrap(), None);
    }

    #[test]
    fn errors_and_nan() {
        assert!(parse_scalar(r#"{"status":"error","error":"bad query"}"#).is_err());
        assert!(parse_scalar("not json").is_err());
        let nan = r#"{"status":"success","data":{"result":[{"metric":{},"value":[1,"NaN"]}]}}"#;
        assert!(parse_scalar(nan).is_err());
    }

    #[test]
    fn counts_are_rounded_and_clamped() {
        assert_eq!(to_count(2.6), 3);
        assert_eq!(to_count(-4.0), 0);
        assert_eq!(to_count(1e20), u32::MAX);
    }
}
