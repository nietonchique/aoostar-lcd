//! Docker Engine API over its unix socket: container health, Woodpecker CI
//! pipelines and the container id -> display name map.
//!
//! Only the `CI_*` keys listed in [`parse_ci_env`] are ever read from a
//! container's environment; everything else (which may hold secrets) is
//! dropped immediately.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

use crate::model::Pipeline;

const IO_TIMEOUT: Duration = Duration::from_secs(3);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
const CI_UUID_LABEL: &str = "wp_uuid";
const CI_STEP_LABEL: &str = "wp_step";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerHealth {
    /// Running, healthy or without a health check.
    Ok,
    /// Running but its health check has not passed yet.
    Starting,
    Unhealthy,
    Restarting,
    /// Created, exited, paused, ...
    Stopped,
}

#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub health: ContainerHealth,
    /// `com.docker.compose.project` label.
    pub project: Option<String>,
    /// Woodpecker step container (`wp_uuid` label present).
    pub ci_step: bool,
    pub ci_step_label: Option<String>,
    /// Unix seconds.
    pub created: i64,
}

impl ContainerInfo {
    /// Subject used for problems: compose project, else container name.
    pub fn subject(&self) -> &str {
        self.project.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DockerState {
    pub containers: Vec<ContainerInfo>,
    pub pipelines: Vec<Pipeline>,
    /// Container id -> display name.
    pub names: HashMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawContainer {
    id: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    labels: Option<HashMap<String, String>>,
    #[serde(default)]
    created: i64,
}

pub fn parse_containers(json: &str) -> anyhow::Result<Vec<ContainerInfo>> {
    let raw: Vec<RawContainer> = serde_json::from_str(json).context("docker container list")?;
    Ok(raw
        .into_iter()
        .map(|container| {
            let labels = container.labels.unwrap_or_default();
            let name = container
                .names
                .first()
                .map(|name| name.trim_start_matches('/').to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| container.id.chars().take(12).collect());
            ContainerInfo {
                running: container.state == "running",
                health: classify_health(&container.state, &container.status),
                project: labels
                    .get(COMPOSE_PROJECT_LABEL)
                    .filter(|project| !project.is_empty())
                    .cloned(),
                ci_step: labels.contains_key(CI_UUID_LABEL),
                ci_step_label: labels.get(CI_STEP_LABEL).cloned(),
                created: container.created,
                id: container.id,
                name,
            }
        })
        .collect())
}

/// `state` is the Engine API state, `status` the human string such as
/// `Up 21 hours (unhealthy)` or `Up 3 seconds (health: starting)`.
pub fn classify_health(state: &str, status: &str) -> ContainerHealth {
    match state {
        "restarting" => ContainerHealth::Restarting,
        "running" if status.contains("(unhealthy)") => ContainerHealth::Unhealthy,
        "running" if status.contains("(health: starting)") => ContainerHealth::Starting,
        "running" => ContainerHealth::Ok,
        _ => ContainerHealth::Stopped,
    }
}

/// The few `CI_*` values needed to describe a pipeline step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CiMeta {
    pub repo: String,
    pub number: u64,
    pub branch: String,
    pub step: String,
    pub started: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawInspect {
    #[serde(default)]
    config: Option<RawInspectConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawInspectConfig {
    #[serde(default)]
    env: Option<Vec<String>>,
}

/// Extracts [`CiMeta`] from a `GET /containers/{id}/json` body. `None` when the
/// container is not a pipeline step (no `CI_REPO`).
pub fn parse_ci_env(inspect_json: &str) -> Option<CiMeta> {
    let inspect: RawInspect = serde_json::from_str(inspect_json).ok()?;
    let env = inspect.config?.env?;
    let mut meta = CiMeta::default();
    for entry in &env {
        let Some((key, value)) = entry.split_once('=') else {
            continue;
        };
        match key {
            "CI_REPO" => meta.repo = value.to_owned(),
            "CI_PIPELINE_NUMBER" => meta.number = value.parse().unwrap_or(0),
            "CI_PIPELINE_STARTED" => meta.started = value.parse().ok().filter(|&t| t > 0),
            "CI_STEP_NAME" => meta.step = value.to_owned(),
            "CI_COMMIT_BRANCH" => meta.branch = value.to_owned(),
            _ => {}
        }
    }
    (!meta.repo.is_empty()).then_some(meta)
}

/// Groups running step containers by (repo, pipeline number), oldest first.
pub fn group_pipelines(steps: &[CiMeta]) -> Vec<Pipeline> {
    let mut groups: BTreeMap<(&str, u64), Pipeline> = BTreeMap::new();
    for step in steps {
        let pipeline = groups
            .entry((step.repo.as_str(), step.number))
            .or_insert_with(|| Pipeline {
                repo: step.repo.clone(),
                number: step.number,
                ..Pipeline::default()
            });
        if pipeline.branch.is_empty() {
            pipeline.branch = step.branch.clone();
        }
        if !step.step.is_empty() && !pipeline.steps.contains(&step.step) {
            pipeline.steps.push(step.step.clone());
        }
        if let Some(started) = step.started
            && (pipeline.started_at == 0 || started < pipeline.started_at)
        {
            pipeline.started_at = started;
        }
    }
    let mut pipelines: Vec<Pipeline> = groups.into_values().collect();
    // Pipelines with an unknown start time sort last.
    let age_key = |pipeline: &Pipeline| match pipeline.started_at {
        0 => i64::MAX,
        started => started,
    };
    pipelines.sort_by(|a, b| {
        age_key(a)
            .cmp(&age_key(b))
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| a.number.cmp(&b.number))
    });
    pipelines
}

/// `azeden/l2-web` -> `CI l2-web`.
pub fn ci_display_name(repo: &str) -> String {
    format!("CI {}", repo.rsplit('/').next().unwrap_or(repo))
}

/// Running container count without the per-container CI inspection. Used by
/// the shutdown page, which keeps polling while the daemon itself goes away.
pub fn running_count(socket: &Path) -> anyhow::Result<u32> {
    let body = get(socket, "/containers/json?all=1")?;
    let containers = parse_containers(&String::from_utf8_lossy(&body))?;
    Ok(containers
        .iter()
        .filter(|container| container.running)
        .count() as u32)
}

pub struct Docker {
    socket: PathBuf,
    /// Container id -> pipeline metadata. Environment never changes for a
    /// container, so each is inspected once.
    cache: HashMap<String, CiMeta>,
}

impl Docker {
    pub fn new(socket: &Path) -> Self {
        Self {
            socket: socket.to_owned(),
            cache: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) -> anyhow::Result<DockerState> {
        let body = get(&self.socket, "/containers/json?all=1").context("docker list")?;
        let containers = parse_containers(&String::from_utf8_lossy(&body))?;

        let live: HashSet<&str> = containers.iter().map(|c| c.id.as_str()).collect();
        self.cache.retain(|id, _| live.contains(id.as_str()));

        let mut steps = Vec::new();
        let mut names = HashMap::with_capacity(containers.len());
        for container in &containers {
            let mut display = container.name.clone();
            if container.ci_step && container.running {
                if !self.cache.contains_key(&container.id)
                    && let Some(meta) = self.inspect(&container.id)
                {
                    self.cache.insert(container.id.clone(), meta);
                }
                if let Some(meta) = self.cache.get(&container.id) {
                    display = ci_display_name(&meta.repo);
                    let mut step = meta.clone();
                    if step.step.is_empty() {
                        step.step = container.ci_step_label.clone().unwrap_or_default();
                    }
                    if step.started.is_none() && container.created > 0 {
                        step.started = Some(container.created);
                    }
                    steps.push(step);
                }
            }
            names.insert(container.id.clone(), display);
        }

        Ok(DockerState {
            containers,
            pipelines: group_pipelines(&steps),
            names,
        })
    }

    fn inspect(&self, id: &str) -> Option<CiMeta> {
        if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let body = get(&self.socket, &format!("/containers/{id}/json")).ok()?;
        parse_ci_env(&String::from_utf8_lossy(&body))
    }
}

/// Minimal HTTP/1.0 GET over the Docker socket; returns the body of a 200.
fn get(socket: &Path, path: &str) -> anyhow::Result<Vec<u8>> {
    let started = Instant::now();
    let mut stream =
        UnixStream::connect(socket).with_context(|| format!("connecting {}", socket.display()))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: docker\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        raw.extend_from_slice(&chunk[..read]);
        if raw.len() as u64 > MAX_RESPONSE {
            bail!("docker response too large");
        }
        if started.elapsed() > TOTAL_TIMEOUT {
            bail!("docker response too slow");
        }
    }
    let (status, body) = parse_http_response(&raw)?;
    if status != 200 {
        bail!("docker HTTP {status}");
    }
    Ok(body)
}

/// Splits a raw HTTP/1.x response into status code and decoded body.
pub fn parse_http_response(raw: &[u8]) -> anyhow::Result<(u16, Vec<u8>)> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("malformed HTTP status line"))?;
    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    });
    let body = if chunked {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    Ok((status, body))
}

fn decode_chunked(mut data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    loop {
        let line_end = data
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("truncated chunked body"))?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).context("bad chunk size")?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            bail!("truncated chunk");
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        data = data.strip_prefix(b"\r\n").unwrap_or(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
{"Id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","Names":["/shop-web-1"],"State":"running","Status":"Up 21 hours (healthy)","Created":1790000000,
 "Labels":{"com.docker.compose.project":"shop"}},
{"Id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","Names":["/shop-db-1"],"State":"running","Status":"Up 21 hours (unhealthy)","Created":1790000000,
 "Labels":{"com.docker.compose.project":"shop"}},
{"Id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","Names":["/worker"],"State":"restarting","Status":"Restarting (1) 5 seconds ago","Created":1790000000,"Labels":null},
{"Id":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","Names":["/cache"],"State":"running","Status":"Up 3 seconds (health: starting)","Created":1790000000,"Labels":{}},
{"Id":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","Names":["/old"],"State":"exited","Status":"Exited (0) 2 days ago","Created":1790000000,"Labels":{}},
{"Id":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","Names":["/wp_01abc"],"State":"running","Status":"Up 2 minutes","Created":1792000000,
 "Labels":{"wp_uuid":"01abc","wp_step":"fast"}}
]"#;

    #[test]
    fn container_list_is_classified() {
        let containers = parse_containers(LIST).unwrap();
        let health: Vec<_> = containers.iter().map(|c| c.health).collect();
        assert_eq!(
            health,
            [
                ContainerHealth::Ok,
                ContainerHealth::Unhealthy,
                ContainerHealth::Restarting,
                ContainerHealth::Starting,
                ContainerHealth::Stopped,
                ContainerHealth::Ok,
            ]
        );
        assert_eq!(containers.iter().filter(|c| c.running).count(), 4);
        assert_eq!(containers[0].subject(), "shop");
        assert_eq!(containers[2].subject(), "worker");
        assert_eq!(containers[2].name, "worker");
        assert!(containers[5].ci_step);
        assert_eq!(containers[5].ci_step_label.as_deref(), Some("fast"));
        assert!(!containers[0].ci_step);
    }

    #[test]
    fn ci_env_reads_only_known_keys() {
        let inspect = r#"{"Id":"x","Config":{"Env":[
"PATH=/usr/bin","CI_REPO=azeden/l2-web","CI_PIPELINE_NUMBER=430","CI_PIPELINE_STARTED=1792000100",
"CI_STEP_NAME=fast","CI_COMMIT_BRANCH=feature/x","DB_PASSWORD=hunter2","MALFORMED"]}}"#;
        let meta = parse_ci_env(inspect).unwrap();
        assert_eq!(
            meta,
            CiMeta {
                repo: "azeden/l2-web".into(),
                number: 430,
                branch: "feature/x".into(),
                step: "fast".into(),
                started: Some(1_792_000_100),
            }
        );
        assert!(parse_ci_env(r#"{"Config":{"Env":["PATH=/bin"]}}"#).is_none());
        assert!(parse_ci_env(r#"{"Config":{"Env":null}}"#).is_none());
        assert!(parse_ci_env("garbage").is_none());
    }

    fn step(repo: &str, number: u64, name: &str, started: Option<i64>) -> CiMeta {
        CiMeta {
            repo: repo.into(),
            number,
            branch: "main".into(),
            step: name.into(),
            started,
        }
    }

    #[test]
    fn steps_group_into_pipelines_oldest_first() {
        let steps = [
            step("azeden/l2-web", 430, "fast", Some(1_792_000_300)),
            step("azeden/l2-web", 430, "slow", Some(1_792_000_100)),
            step("azeden/l2-web", 430, "slow", Some(1_792_000_100)),
            step("core/hub", 12, "build", Some(1_792_000_000)),
            step("azeden/l2-web", 431, "fast", None),
        ];
        let pipelines = group_pipelines(&steps);
        assert_eq!(pipelines.len(), 3);
        assert_eq!(pipelines[0].repo, "core/hub");
        assert_eq!(pipelines[1].repo, "azeden/l2-web");
        assert_eq!(pipelines[1].number, 430);
        assert_eq!(pipelines[1].steps, ["fast", "slow"]);
        assert_eq!(pipelines[1].started_at, 1_792_000_100);
        assert_eq!(pipelines[1].branch, "main");
        // Unknown start time sorts last.
        assert_eq!(pipelines[2].number, 431);
    }

    #[test]
    fn ci_names_use_repo_basename() {
        assert_eq!(ci_display_name("azeden/l2-web"), "CI l2-web");
        assert_eq!(ci_display_name("solo"), "CI solo");
    }

    #[test]
    fn http_response_plain_and_chunked() {
        let plain = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n[1,2]";
        assert_eq!(
            parse_http_response(plain).unwrap(),
            (200, b"[1,2]".to_vec())
        );

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n[1,\r\n2\r\n2]\r\n0\r\n\r\n";
        assert_eq!(
            parse_http_response(chunked).unwrap(),
            (200, b"[1,2]".to_vec())
        );

        let missing = b"HTTP/1.0 404 Not Found\r\n\r\n{\"message\":\"no such container\"}";
        assert_eq!(parse_http_response(missing).unwrap().0, 404);
        assert!(parse_http_response(b"garbage").is_err());
        assert!(
            parse_http_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
                .is_err()
        );
    }
}
