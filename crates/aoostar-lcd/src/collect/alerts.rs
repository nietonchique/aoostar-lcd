//! Active alerts from Grafana's built-in Alertmanager API.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;

use crate::config::AlertsConfig;
use crate::model::Severity;

/// Reason string used when the token file is missing or empty.
pub const NO_TOKEN: &str = "no token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub alertname: String,
    /// `labels.service`.
    pub service: Option<String>,
    /// `labels.alert_family`.
    pub family: Option<String>,
    /// `labels.node`.
    pub node: Option<String>,
    pub severity: Severity,
    /// `annotations.summary`.
    pub summary: Option<String>,
    /// Unix seconds.
    pub starts_at: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FetchError {
    NoToken,
    Failed(String),
}

impl FetchError {
    pub fn reason(&self) -> String {
        match self {
            Self::NoToken => NO_TOKEN.to_owned(),
            Self::Failed(reason) => reason.clone(),
        }
    }
}

#[derive(Deserialize)]
struct RawAlert {
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    annotations: HashMap<String, String>,
    #[serde(default, rename = "startsAt")]
    starts_at: Option<String>,
    #[serde(default)]
    status: Option<RawStatus>,
}

#[derive(Deserialize)]
struct RawStatus {
    #[serde(default)]
    state: Option<String>,
}

pub fn fetch(config: &AlertsConfig) -> Result<Vec<Alert>, FetchError> {
    let token = fs::read_to_string(&config.token_file)
        .map(|text| text.trim().to_owned())
        .ok()
        .filter(|token| !token.is_empty())
        .ok_or(FetchError::NoToken)?;

    let url = format!(
        "{}/api/alertmanager/grafana/api/v2/alerts",
        config.grafana_url.trim_end_matches('/')
    );
    let mut request = super::http_agent()
        .get(&url)
        .query("active", "true")
        .query("silenced", "false")
        .query("inhibited", "false")
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json");
    if let Some(host) = &config.grafana_host {
        // Grafana redirects requests whose Host is not its root URL.
        request = request.set("Host", host);
    }
    let response = request.call().map_err(|error| match error {
        ureq::Error::Status(code, _) => FetchError::Failed(format!("HTTP {code}")),
        ureq::Error::Transport(transport) => FetchError::Failed(transport.kind().to_string()),
    })?;
    let body = response
        .into_string()
        .map_err(|_| FetchError::Failed("read error".to_owned()))?;
    parse_alerts(&body).map_err(|_| FetchError::Failed("bad response".to_owned()))
}

/// Parses the Alertmanager v2 alert list, keeping only active alerts.
pub fn parse_alerts(json: &str) -> serde_json::Result<Vec<Alert>> {
    let raw: Vec<RawAlert> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .filter(|alert| {
            alert
                .status
                .as_ref()
                .and_then(|status| status.state.as_deref())
                .is_none_or(|state| state == "active")
        })
        .map(|mut alert| {
            let mut label = |key: &str| {
                alert
                    .labels
                    .remove(key)
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            };
            let alertname = label("alertname").unwrap_or_else(|| "alert".to_owned());
            let service = label("service");
            let family = label("alert_family");
            let node = label("node");
            let severity = match label("severity").as_deref() {
                Some("critical") => Severity::Critical,
                _ => Severity::Warning,
            };
            let summary = alert
                .annotations
                .remove("summary")
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            Alert {
                alertname,
                service,
                family,
                node,
                severity,
                summary,
                starts_at: alert.starts_at.as_deref().and_then(parse_rfc3339),
            }
        })
        .collect())
}

/// Parses `2026-10-19T08:30:00Z`, `2026-10-19T11:30:00.123+03:00` and similar
/// into unix seconds.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let text = text.trim();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let separator =
        |at: usize, allowed: &[u8]| text.as_bytes().get(at).is_some_and(|b| allowed.contains(b));
    if !(separator(4, b"-")
        && separator(7, b"-")
        && separator(10, b"Tt ")
        && separator(13, b":")
        && separator(16, b":"))
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = &fraction[digits..];
    }
    let offset_secs = match rest.as_bytes() {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), tail @ ..] => {
            let tail = std::str::from_utf8(tail).ok()?;
            let (hours, minutes) = match tail.split_once(':') {
                Some(parts) => parts,
                None if tail.len() == 4 && tail.is_ascii() => tail.split_at(2),
                None => return None,
            };
            let hours: i64 = hours.parse().ok()?;
            let minutes: i64 = minutes.parse().ok()?;
            let magnitude = hours * 3600 + minutes * 60;
            if *sign == b'-' { -magnitude } else { magnitude }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs)
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_utc_and_offsets() {
        // 2026-10-19T08:30:00Z
        let utc = parse_rfc3339("2026-10-19T08:30:00Z").unwrap();
        assert_eq!(utc, 1_792_398_600);
        // Same instant expressed in +03:00 and -05:30.
        assert_eq!(parse_rfc3339("2026-10-19T11:30:00+03:00"), Some(utc));
        assert_eq!(parse_rfc3339("2026-10-19T03:00:00-05:30"), Some(utc));
        assert_eq!(parse_rfc3339("2026-10-19T11:30:00+0300"), Some(utc));
        // Fractional seconds are ignored.
        assert_eq!(parse_rfc3339("2026-10-19T08:30:00.123456789Z"), Some(utc));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
    }

    #[test]
    fn rfc3339_rejects_garbage() {
        for bad in [
            "",
            "yesterday",
            "2026-10-19",
            "2026-13-19T00:00:00Z",
            "2026-10-19T08:30:00",
            "2026-10-19T08:30:00X",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    const ALERTS: &str = r#"[
{"labels":{"alertname":"BackupStale","severity":"warning","service":"urbackup","alert_family":"backup-freshness","node":"computer-granite"},
 "annotations":{"summary":"Backup for client X is stale"},"startsAt":"2026-10-19T08:30:00Z","status":{"state":"active"}},
{"labels":{"alertname":"ExpectedContainerMissingWarning","severity":"warning","service":"cars","alert_family":"docker-state"},
 "annotations":{},"startsAt":"2026-10-19T11:30:00+03:00","status":{"state":"active"}},
{"labels":{"alertname":"Silenced","severity":"critical","service":"x"},"annotations":{},"startsAt":"2026-10-19T08:30:00Z","status":{"state":"suppressed"}},
{"labels":{"severity":"critical"},"annotations":{"summary":""},"startsAt":"nonsense"}
]"#;

    #[test]
    fn alert_json_parsing() {
        let alerts = parse_alerts(ALERTS).unwrap();
        // The suppressed alert is dropped.
        assert_eq!(alerts.len(), 3);
        assert_eq!(alerts[0].alertname, "BackupStale");
        assert_eq!(alerts[0].service.as_deref(), Some("urbackup"));
        assert_eq!(alerts[0].family.as_deref(), Some("backup-freshness"));
        assert_eq!(alerts[0].node.as_deref(), Some("computer-granite"));
        assert_eq!(
            alerts[0].summary.as_deref(),
            Some("Backup for client X is stale")
        );
        assert_eq!(alerts[0].starts_at, Some(1_792_398_600));
        assert_eq!(alerts[1].starts_at, Some(1_792_398_600));
        assert_eq!(alerts[1].node, None);
        assert_eq!(alerts[1].summary, None);
        assert_eq!(alerts[2].alertname, "alert");
        assert_eq!(alerts[2].severity, Severity::Critical);
        assert_eq!(alerts[2].starts_at, None);
        assert_eq!(alerts[2].summary, None);
        assert!(parse_alerts("{}").is_err());
        assert!(parse_alerts("[]").unwrap().is_empty());
    }
}
