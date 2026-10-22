//! Turns alerts and local checks into the sorted problem list the status bar
//! and problem pages show. Pure functions over already-collected data.

use std::collections::HashMap;

use crate::model::{Health, Pool, Problem, ProblemClass, Severity, SourceState};

use super::alerts::{Alert, NO_TOKEN};
use super::docker::{ContainerHealth, DockerState};
use super::systemd::{SystemdState, UnitState};

/// What the Grafana source currently offers.
#[derive(Debug, Clone, Copy)]
pub enum AlertsInput<'a> {
    /// `[alerts]` is not configured.
    Disabled,
    /// No fetch has completed yet.
    Pending,
    /// Token file missing or unreadable.
    NoToken,
    /// Grafana could not be reached or answered garbage.
    Unavailable(&'a str),
    Active(&'a [Alert]),
}

pub struct Inputs<'a> {
    pub alerts: AlertsInput<'a>,
    pub local_node: Option<&'a str>,
    pub outage_families: &'a [String],
    pub systemd: Option<&'a SystemdState>,
    pub docker: Option<&'a DockerState>,
    pub pools: &'a [Pool],
}

pub fn build(inputs: &Inputs) -> Health {
    let mut problems = local_problems(inputs);

    let alerts = match inputs.alerts {
        AlertsInput::Disabled => SourceState::Disabled,
        AlertsInput::Pending => SourceState::Unavailable("starting".to_owned()),
        AlertsInput::NoToken => SourceState::Unavailable(NO_TOKEN.to_owned()),
        AlertsInput::Unavailable(reason) => {
            problems.push(Problem {
                class: ProblemClass::Outage,
                severity: Severity::Warning,
                subject: "grafana".to_owned(),
                detail: "alerts unavailable".to_owned(),
                since: None,
                count: 1,
            });
            SourceState::Unavailable(reason.to_owned())
        }
        AlertsInput::Active(alerts) => {
            problems.extend(alert_problems(
                alerts,
                inputs.local_node,
                inputs.outage_families,
            ));
            SourceState::Ok
        }
    };

    let (containers_running, containers_total) = inputs.docker.map_or((0, 0), |docker| {
        (
            docker.containers.iter().filter(|c| c.running).count() as u32,
            docker.containers.len() as u32,
        )
    });

    Health {
        problems: merge_and_sort(problems),
        alerts,
        containers_running,
        containers_total,
    }
}

/// `computer-granite` -> `granite`.
pub fn short_node(node: &str) -> &str {
    node.strip_prefix("computer-").unwrap_or(node)
}

/// One problem per subject from the firing Grafana alerts.
pub fn alert_problems(
    alerts: &[Alert],
    local_node: Option<&str>,
    outage_families: &[String],
) -> Vec<Problem> {
    // Subject -> alerts, in first-appearance order.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&Alert>> = HashMap::new();
    for alert in alerts {
        let base = alert.service.as_deref().unwrap_or(&alert.alertname);
        let subject = match (alert.node.as_deref(), local_node) {
            (Some(node), Some(local)) if node != local => {
                format!("{} · {base}", short_node(node))
            }
            _ => base.to_owned(),
        };
        groups
            .entry(subject.clone())
            .or_insert_with(|| {
                order.push(subject);
                Vec::new()
            })
            .push(alert);
    }

    order
        .into_iter()
        .filter_map(|subject| {
            let group = groups.remove(&subject)?;
            let outage = group.iter().any(|alert| {
                alert
                    .family
                    .as_ref()
                    .is_some_and(|family| outage_families.contains(family))
            });
            let detail = if let [only] = group.as_slice() {
                only.summary
                    .clone()
                    .unwrap_or_else(|| only.alertname.clone())
            } else {
                format!("{} alerts: {}", group.len(), most_frequent_name(&group))
            };
            Some(Problem {
                class: if outage {
                    ProblemClass::Outage
                } else {
                    ProblemClass::Alert
                },
                severity: group
                    .iter()
                    .map(|alert| alert.severity)
                    .max()
                    .unwrap_or_default(),
                subject,
                detail,
                since: group.iter().filter_map(|alert| alert.starts_at).min(),
                count: group.len() as u32,
            })
        })
        .collect()
}

/// Most frequent alert name; ties go to the alphabetically first.
fn most_frequent_name<'a>(group: &[&'a Alert]) -> &'a str {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for alert in group {
        *counts.entry(alert.alertname.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or("", |(name, _)| name)
}

fn local_problem(subject: &str, detail: &str, since: Option<i64>, maintenance: bool) -> Problem {
    Problem {
        class: ProblemClass::Outage,
        severity: if maintenance {
            Severity::Maintenance
        } else {
            Severity::Critical
        },
        subject: subject.to_owned(),
        detail: if maintenance { "maintenance" } else { detail }.to_owned(),
        since,
        count: 1,
    }
}

/// Checks derived from this host's own state.
fn local_problems(inputs: &Inputs) -> Vec<Problem> {
    let mut problems = Vec::new();
    let in_maintenance = |subject: &str| inputs.systemd.is_some_and(|s| s.in_maintenance(subject));

    if let Some(systemd) = inputs.systemd {
        for stack in systemd.stacks.iter().filter(|unit| !unit.is_active()) {
            problems.push(local_problem(
                &stack.subject,
                "stack inactive",
                stack.since,
                in_maintenance(&stack.subject),
            ));
        }
        for unit in systemd.units.iter().filter(|unit| !unit.is_active()) {
            let detail = if unit.active == "failed" {
                "unit failed"
            } else if unit.load == "not-found" {
                "unit missing"
            } else {
                "unit inactive"
            };
            problems.push(local_problem(
                &unit.subject,
                detail,
                unit.since,
                in_maintenance(&unit.subject),
            ));
        }
        // Failed services are outages; failed run-to-completion jobs (backups,
        // health probes, reconcilers) are one grouped alert: they need
        // attention but nothing is down because of them.
        let mut failed_jobs: Vec<&UnitState> = Vec::new();
        for unit in &systemd.failed {
            if unit.job {
                failed_jobs.push(unit);
                continue;
            }
            problems.push(local_problem(
                &unit.subject,
                "unit failed",
                unit.since,
                in_maintenance(&unit.subject),
            ));
        }
        if !failed_jobs.is_empty() {
            let names: Vec<&str> = failed_jobs
                .iter()
                .map(|unit| unit.subject.as_str())
                .collect();
            problems.push(Problem {
                class: ProblemClass::Alert,
                severity: Severity::Critical,
                subject: "failed jobs".to_owned(),
                detail: names.join(", "),
                since: failed_jobs.iter().filter_map(|unit| unit.since).min(),
                count: failed_jobs.len() as u32,
            });
        }
    }

    if let Some(docker) = inputs.docker {
        // CI step containers are ephemeral build jobs; a failing healthcheck
        // inside a test image is not a Granite service being down.
        for container in docker.containers.iter().filter(|c| !c.ci_step) {
            let detail = match container.health {
                ContainerHealth::Unhealthy => "container unhealthy",
                ContainerHealth::Restarting => "container restarting",
                _ => continue,
            };
            let subject = container.subject();
            problems.push(local_problem(
                subject,
                detail,
                None,
                in_maintenance(subject),
            ));
        }
    }

    for pool in inputs.pools {
        if pool.health != "ONLINE" {
            problems.push(local_problem(
                &pool.name,
                &format!("pool {}", pool.health),
                None,
                false,
            ));
        } else if pool.errors > 0 {
            problems.push(local_problem(&pool.name, "pool errors", None, false));
        }
    }
    problems
}

/// Merges problems with equal subjects (case-insensitive) and sorts them:
/// active outages, maintenance outages, then alerts; severity descending, then
/// oldest first.
pub fn merge_and_sort(problems: Vec<Problem>) -> Vec<Problem> {
    struct Merged {
        problem: Problem,
        /// Rank of the item whose subject/detail is currently kept.
        rank: (Severity, bool),
    }
    let rank = |problem: &Problem| (problem.severity, problem.class == ProblemClass::Outage);

    let mut merged: Vec<Merged> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for problem in problems {
        let key = problem.subject.to_lowercase();
        match index.get(&key) {
            None => {
                index.insert(key, merged.len());
                merged.push(Merged {
                    rank: rank(&problem),
                    problem,
                });
            }
            Some(&at) => {
                let slot = &mut merged[at];
                let incoming_rank = rank(&problem);
                let target = &mut slot.problem;
                if problem.class == ProblemClass::Outage {
                    target.class = ProblemClass::Outage;
                }
                target.severity = target.severity.max(problem.severity);
                target.count = target.count.saturating_add(problem.count);
                target.since = match (target.since, problem.since) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                if incoming_rank > slot.rank {
                    target.subject = problem.subject;
                    target.detail = problem.detail;
                    slot.rank = incoming_rank;
                }
            }
        }
    }

    let mut problems: Vec<Problem> = merged.into_iter().map(|m| m.problem).collect();
    let group = |problem: &Problem| match (problem.class, problem.severity) {
        (ProblemClass::Outage, Severity::Maintenance) => 1u8,
        (ProblemClass::Outage, _) => 0,
        (ProblemClass::Alert, _) => 2,
    };
    problems.sort_by(|a, b| {
        group(a)
            .cmp(&group(b))
            .then_with(|| b.severity.cmp(&a.severity))
            .then_with(|| {
                a.since
                    .unwrap_or(i64::MAX)
                    .cmp(&b.since.unwrap_or(i64::MAX))
            })
            .then_with(|| a.subject.cmp(&b.subject))
    });
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::docker::ContainerInfo;

    fn alert(
        name: &str,
        service: Option<&str>,
        family: &str,
        severity: Severity,
        at: i64,
    ) -> Alert {
        Alert {
            alertname: name.to_owned(),
            service: service.map(str::to_owned),
            family: Some(family.to_owned()),
            node: None,
            severity,
            summary: None,
            starts_at: Some(at),
        }
    }

    fn families() -> Vec<String> {
        ["docker-state", "availability"].map(str::to_owned).to_vec()
    }

    fn inputs<'a>(alerts: AlertsInput<'a>, families: &'a [String]) -> Inputs<'a> {
        Inputs {
            alerts,
            local_node: Some("computer-granite"),
            outage_families: families,
            systemd: None,
            docker: None,
            pools: &[],
        }
    }

    fn unit(subject: &str, active: &str, since: Option<i64>) -> UnitState {
        UnitState {
            subject: subject.to_owned(),
            load: "loaded".to_owned(),
            active: active.to_owned(),
            since,
            job: false,
        }
    }

    #[test]
    fn failed_jobs_are_one_alert_not_an_outage() {
        let families = families();
        let job = |subject: &str, since| UnitState {
            job: true,
            ..unit(subject, "failed", Some(since))
        };
        let systemd = SystemdState {
            failed: vec![
                job("granite-immich-library-backup", 2000),
                job("reverse-proxy-health", 1000),
                unit("smb", "failed", Some(3000)),
            ],
            ..SystemdState::default()
        };
        let mut input = inputs(AlertsInput::Active(&[]), &families);
        input.systemd = Some(&systemd);
        let health = build(&input);

        assert_eq!(health.active_outages().count(), 1);
        assert_eq!(health.active_outages().next().unwrap().subject, "smb");
        let jobs: Vec<_> = health.alerts().collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].count, 2);
        assert_eq!(jobs[0].since, Some(1000));
        assert_eq!(
            jobs[0].detail,
            "granite-immich-library-backup, reverse-proxy-health"
        );
    }

    fn container(name: &str, project: Option<&str>, health: ContainerHealth) -> ContainerInfo {
        ContainerInfo {
            id: name.to_owned(),
            name: name.to_owned(),
            running: health != ContainerHealth::Stopped,
            health,
            project: project.map(str::to_owned),
            ci_step: false,
            ci_step_label: None,
            created: 0,
        }
    }

    #[test]
    fn unhealthy_ci_step_container_is_not_an_outage() {
        let families = families();
        let docker = DockerState {
            containers: vec![ContainerInfo {
                ci_step: true,
                ..container("wp_01ABC", None, ContainerHealth::Unhealthy)
            }],
            ..DockerState::default()
        };
        let mut input = inputs(AlertsInput::Active(&[]), &families);
        input.docker = Some(&docker);
        assert!(build(&input).problems.is_empty());
    }

    #[test]
    fn single_alert_uses_summary_then_alertname() {
        let mut with_summary = alert(
            "DiskFull",
            Some("cars"),
            "docker-state",
            Severity::Warning,
            100,
        );
        with_summary.summary = Some("Container cars missing".to_owned());
        let bare = alert(
            "PostgresDeadlocks",
            Some("postgresql"),
            "postgresql",
            Severity::Warning,
            100,
        );
        let problems = alert_problems(&[with_summary, bare], Some("computer-granite"), &families());
        assert_eq!(problems.len(), 2);
        assert_eq!(problems[0].subject, "cars");
        assert_eq!(problems[0].detail, "Container cars missing");
        assert_eq!(problems[0].class, ProblemClass::Outage);
        assert_eq!(problems[0].count, 1);
        assert_eq!(problems[1].detail, "PostgresDeadlocks");
        assert_eq!(problems[1].class, ProblemClass::Alert);
    }

    #[test]
    fn several_alerts_group_by_service() {
        let mut alerts: Vec<Alert> = (0..3)
            .map(|i| {
                alert(
                    "BackupStale",
                    Some("urbackup"),
                    "backup-freshness",
                    Severity::Warning,
                    300 - i,
                )
            })
            .collect();
        alerts.push(alert(
            "BackupCapacity",
            Some("urbackup"),
            "backup-capacity",
            Severity::Critical,
            500,
        ));
        alerts.push(alert("NoService", None, "misc", Severity::Warning, 50));
        let problems = alert_problems(&alerts, Some("computer-granite"), &families());
        assert_eq!(problems.len(), 2);
        let urbackup = &problems[0];
        assert_eq!(urbackup.subject, "urbackup");
        assert_eq!(urbackup.detail, "4 alerts: BackupStale");
        assert_eq!(urbackup.count, 4);
        assert_eq!(urbackup.severity, Severity::Critical);
        assert_eq!(urbackup.since, Some(298));
        assert_eq!(urbackup.class, ProblemClass::Alert);
        // Missing service falls back to the alert name.
        assert_eq!(problems[1].subject, "NoService");
    }

    #[test]
    fn foreign_nodes_are_prefixed() {
        let mut remote = alert(
            "FilesystemPredictedFull",
            Some("filesystem"),
            "filesystem",
            Severity::Warning,
            10,
        );
        remote.node = Some("computer-obsidian".to_owned());
        let mut local = alert(
            "FilesystemPredictedFull",
            Some("filesystem"),
            "filesystem",
            Severity::Warning,
            20,
        );
        local.node = Some("computer-granite".to_owned());
        let problems = alert_problems(
            &[remote.clone(), local],
            Some("computer-granite"),
            &families(),
        );
        let subjects: Vec<_> = problems.iter().map(|p| p.subject.as_str()).collect();
        assert_eq!(subjects, ["obsidian · filesystem", "filesystem"]);
        // Without a configured local node nothing is prefixed.
        let problems = alert_problems(&[remote], None, &families());
        assert_eq!(problems[0].subject, "filesystem");
    }

    #[test]
    fn merge_rules() {
        let problems = vec![
            Problem {
                class: ProblemClass::Alert,
                severity: Severity::Warning,
                subject: "Cars".into(),
                detail: "slow".into(),
                since: Some(200),
                count: 2,
            },
            Problem {
                class: ProblemClass::Outage,
                severity: Severity::Critical,
                subject: "cars".into(),
                detail: "container unhealthy".into(),
                since: Some(300),
                count: 1,
            },
            Problem {
                class: ProblemClass::Alert,
                severity: Severity::Warning,
                subject: "CARS".into(),
                detail: "other".into(),
                since: None,
                count: 4,
            },
        ];
        let merged = merge_and_sort(problems);
        assert_eq!(merged.len(), 1);
        let problem = &merged[0];
        assert_eq!(problem.class, ProblemClass::Outage);
        assert_eq!(problem.severity, Severity::Critical);
        assert_eq!(problem.since, Some(200));
        assert_eq!(problem.count, 7);
        assert_eq!(problem.detail, "container unhealthy");
        assert_eq!(problem.subject, "cars");
    }

    #[test]
    fn ordering_outages_maintenance_alerts() {
        let make = |class, severity, subject: &str, since| Problem {
            class,
            severity,
            subject: subject.to_owned(),
            detail: String::new(),
            since,
            count: 1,
        };
        let sorted = merge_and_sort(vec![
            make(
                ProblemClass::Alert,
                Severity::Critical,
                "alert-crit",
                Some(1),
            ),
            make(
                ProblemClass::Outage,
                Severity::Maintenance,
                "maint",
                Some(1),
            ),
            make(
                ProblemClass::Outage,
                Severity::Warning,
                "outage-warn-old",
                Some(10),
            ),
            make(
                ProblemClass::Outage,
                Severity::Critical,
                "outage-crit-new",
                Some(900),
            ),
            make(
                ProblemClass::Outage,
                Severity::Warning,
                "outage-warn-new",
                Some(50),
            ),
            make(ProblemClass::Alert, Severity::Warning, "alert-warn", None),
            make(
                ProblemClass::Outage,
                Severity::Critical,
                "outage-crit-old",
                Some(100),
            ),
        ]);
        let subjects: Vec<_> = sorted.iter().map(|p| p.subject.as_str()).collect();
        assert_eq!(
            subjects,
            [
                "outage-crit-old",
                "outage-crit-new",
                "outage-warn-old",
                "outage-warn-new",
                "maint",
                "alert-crit",
                "alert-warn"
            ]
        );
    }

    #[test]
    fn grafana_states() {
        let families = families();
        let health = build(&inputs(AlertsInput::Unavailable("HTTP 502"), &families));
        assert_eq!(health.alerts, SourceState::Unavailable("HTTP 502".into()));
        assert_eq!(health.problems.len(), 1);
        let problem = &health.problems[0];
        assert_eq!(problem.subject, "grafana");
        assert_eq!(problem.class, ProblemClass::Outage);
        assert_eq!(problem.severity, Severity::Warning);
        assert_eq!(problem.detail, "alerts unavailable");

        let health = build(&inputs(AlertsInput::NoToken, &families));
        assert_eq!(health.alerts, SourceState::Unavailable("no token".into()));
        assert!(health.problems.is_empty());

        let health = build(&inputs(AlertsInput::Disabled, &families));
        assert_eq!(health.alerts, SourceState::Disabled);
        assert!(health.problems.is_empty());

        let health = build(&inputs(AlertsInput::Active(&[]), &families));
        assert_eq!(health.alerts, SourceState::Ok);
    }

    #[test]
    fn local_checks_and_maintenance() {
        let families = families();
        let systemd = SystemdState {
            stacks: vec![
                unit("adengard", "active", None),
                unit("dockge", "inactive", Some(1000)),
                unit("shop", "failed", Some(2000)),
            ],
            units: vec![
                unit("docker", "active", None),
                unit("smb", "inactive", Some(3000)),
            ],
            failed: vec![
                unit("smb", "failed", Some(3500)),
                unit("shop", "failed", Some(1500)),
            ],
            maintenance: ["dockge".to_owned()].into_iter().collect(),
        };

        let docker = DockerState {
            containers: vec![
                container("web-1", Some("shop"), ContainerHealth::Unhealthy),
                container("worker", None, ContainerHealth::Restarting),
                container("ok", Some("adengard"), ContainerHealth::Ok),
                container("old", None, ContainerHealth::Stopped),
            ],
            ..DockerState::default()
        };
        let pool = Pool {
            name: "storage01".into(),
            health: "DEGRADED".into(),
            ..Pool::default()
        };

        let mut input = inputs(AlertsInput::Active(&[]), &families);
        input.systemd = Some(&systemd);
        input.docker = Some(&docker);
        input.pools = std::slice::from_ref(&pool);
        let health = build(&input);

        assert_eq!(health.containers_total, 4);
        assert_eq!(health.containers_running, 3);
        let by_subject: HashMap<_, _> = health
            .problems
            .iter()
            .map(|p| (p.subject.as_str(), p))
            .collect();
        assert_eq!(by_subject.len(), 5);
        // shop: stack failed (since 2000), failed unit (since 1500), unhealthy container.
        let shop = by_subject["shop"];
        assert_eq!(shop.count, 3);
        assert_eq!(shop.since, Some(1500));
        assert_eq!(shop.severity, Severity::Critical);
        assert_eq!(shop.detail, "stack inactive");
        assert_eq!(by_subject["smb"].count, 2);
        assert_eq!(by_subject["worker"].detail, "container restarting");
        assert_eq!(by_subject["storage01"].detail, "pool DEGRADED");
        assert_eq!(by_subject["dockge"].severity, Severity::Maintenance);
        assert_eq!(by_subject["dockge"].detail, "maintenance");
        assert!(!by_subject.contains_key("adengard"));
        assert!(!by_subject.contains_key("old"));
        // Maintenance outage sorts after the active ones.
        assert_eq!(
            health.problems.last().map(|p| p.subject.as_str()),
            Some("dockge")
        );
    }

    #[test]
    fn pool_errors_without_degraded_state() {
        let families = families();
        let pool = Pool {
            name: "storage01".into(),
            health: "ONLINE".into(),
            errors: 3,
            ..Pool::default()
        };
        let mut input = inputs(AlertsInput::Active(&[]), &families);
        input.pools = std::slice::from_ref(&pool);
        let health = build(&input);
        assert_eq!(health.problems.len(), 1);
        assert_eq!(health.problems[0].detail, "pool errors");

        let healthy = Pool {
            errors: 0,
            ..pool.clone()
        };
        input.pools = std::slice::from_ref(&healthy);
        assert!(build(&input).problems.is_empty());
    }
}
