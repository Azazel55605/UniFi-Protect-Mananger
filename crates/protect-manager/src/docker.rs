//! Docker client: container discovery, inspection, and log streaming.
//!
//! Only this process talks to the Docker socket, and only over a read-only bind
//! mount — never a network port. The Docker API is root-equivalent on the
//! host, so the socket never leaves this container.

use bollard::query_parameters::{
    InspectContainerOptions, ListContainersOptionsBuilder, LogsOptionsBuilder,
};
use bollard::Docker;
use protect_api_types::{ContainerRef, MountInfo, ProposedConfig, UpbInspection};

/// Environment variables we are willing to read out of the UPB container.
///
/// Everything else stays where it is. The container's environment holds
/// `UFP_USERNAME` and `UFP_PASSWORD` in plaintext, so an allowlist is the only
/// safe shape here: a blocklist would leak whatever a future release of the
/// backup service adds. Nothing outside this list is read, returned, persisted
/// or logged.
const ENV_ALLOWLIST: &[&str] = &["SQLITE_PATH", "TZ"];

pub fn connect() -> anyhow::Result<Docker> {
    Ok(Docker::connect_with_defaults()?)
}

fn clean_name(names: &[String]) -> String {
    names
        .first()
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_default()
}

/// Find candidate backup containers by image.
///
/// Matching on image rather than name is deliberate: a compose deployment that
/// sets no `container_name` gets a generated one, so any name hardcoded here
/// would be wrong on someone else's machine.
pub async fn discover(docker: &Docker, image_needle: &str) -> anyhow::Result<Vec<ContainerRef>> {
    let opts = ListContainersOptionsBuilder::default().all(true).build();
    let needle = image_needle.to_ascii_lowercase();

    Ok(docker
        .list_containers(Some(opts))
        .await?
        .into_iter()
        .filter_map(|c| {
            let image = c.image.clone().unwrap_or_default();
            image
                .to_ascii_lowercase()
                .contains(&needle)
                .then(|| ContainerRef {
                    id: c.id.clone().unwrap_or_default(),
                    name: clean_name(c.names.as_deref().unwrap_or(&[])),
                    image,
                    state: c.state.map(|s| s.to_string()),
                })
        })
        .collect())
}

/// Resolve the container to work with: explicit override, else discovery.
pub async fn resolve(
    docker: &Docker,
    explicit: Option<&str>,
    image_needle: &str,
) -> anyhow::Result<Option<ContainerRef>> {
    if let Some(name) = explicit {
        let inspected = docker.inspect_container(name, None::<InspectContainerOptions>).await?;
        return Ok(Some(ContainerRef {
            id: inspected.id.clone().unwrap_or_default(),
            name: inspected
                .name
                .clone()
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            image: inspected.config.as_ref().and_then(|c| c.image.clone()).unwrap_or_default(),
            state: inspected
                .state
                .as_ref()
                .and_then(|s| s.status.as_ref())
                .map(|s| s.to_string()),
        }));
    }
    Ok(discover(docker, image_needle).await?.into_iter().next())
}

/// Pull a `--flag value` or `--flag=value` out of a token list.
fn flag_value(tokens: &[String], flag: &str) -> Option<String> {
    let mut iter = tokens.iter().peekable();
    while let Some(tok) = iter.next() {
        if let Some(rest) = tok.strip_prefix(&format!("{flag}=")) {
            return Some(rest.trim().to_string());
        }
        if tok == flag {
            if let Some(next) = iter.peek() {
                return Some(next.trim().to_string());
            }
        }
    }
    None
}

/// Split a command that may have arrived as one unsplit string.
///
/// Compose accepts `command:` as either a list or a block scalar; with the
/// latter the whole invocation can land in a single argv entry.
fn tokenize(raw: &[String]) -> Vec<String> {
    raw.iter()
        .flat_map(|s| s.split_whitespace().map(str::to_string))
        .collect()
}

/// Resolve a container path to a host path using the container's own mounts,
/// preferring the most specific mount when several could match.
fn to_host_path(mounts: &[MountInfo], container_path: &str) -> Option<String> {
    mounts
        .iter()
        .filter(|m| m.source.is_some())
        .filter(|m| {
            container_path == m.destination
                || container_path.starts_with(&format!("{}/", m.destination.trim_end_matches('/')))
        })
        .max_by_key(|m| m.destination.len())
        .map(|m| {
            let dest = m.destination.trim_end_matches('/');
            let rest = container_path.strip_prefix(dest).unwrap_or("");
            format!("{}{}", m.source.clone().unwrap().trim_end_matches('/'), rest)
        })
}

/// Inspect the backup container and derive the configuration setup will offer.
///
/// `local_backup_dir` is where *this* container sees the backup root, which is
/// what turns UPB's view of a path into one we can actually open.
pub async fn inspect(
    docker: &Docker,
    container: ContainerRef,
    local_backup_dir: &std::path::Path,
) -> anyhow::Result<UpbInspection> {
    let raw = docker
        .inspect_container(&container.id, None::<InspectContainerOptions>)
        .await?;

    let mounts: Vec<MountInfo> = raw
        .mounts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            m.destination.map(|destination| MountInfo {
                source: m.source,
                destination,
                rw: m.rw.unwrap_or(false),
            })
        })
        .collect();

    let cfg = raw.config.unwrap_or_default();
    let all_env = cfg.env.unwrap_or_default();
    let mut env = Vec::new();
    let mut withheld = 0usize;
    for entry in &all_env {
        match entry.split_once('=') {
            Some((k, v)) if ENV_ALLOWLIST.contains(&k) => env.push((k.to_string(), v.to_string())),
            _ => withheld += 1,
        }
    }

    // `Config.Cmd` and the top-level `Args` describe the same argument vector,
    // so reading both concatenates the command with itself. Prefer Cmd and fall
    // back to Args only when it is absent.
    let mut tokens = tokenize(&cfg.cmd.unwrap_or_default());
    if tokens.is_empty() {
        tokens = tokenize(&raw.args.unwrap_or_default());
    }
    let command = (!tokens.is_empty()).then(|| tokens.join(" "));

    let state = raw.state.unwrap_or_default();

    let sqlite_container_path = env
        .iter()
        .find(|(k, _)| k == "SQLITE_PATH")
        .map(|(_, v)| v.clone());

    let mut notes = Vec::new();
    let sqlite_host_path = sqlite_container_path
        .as_deref()
        .and_then(|p| to_host_path(&mounts, p));
    if sqlite_container_path.is_none() {
        notes.push("SQLITE_PATH not set on the container; database location must be entered manually".into());
    } else if sqlite_host_path.is_none() {
        notes.push("SQLITE_PATH is not inside any bind mount, so it is not reachable from this container".into());
    }

    // UPB writes clips under the container path that shows up in
    // `backups.path`. `/data` is the upstream default; if the deployment moved
    // it, the user corrects this in the setup flow.
    let clip_mount = mounts.iter().find(|m| m.destination == "/data");
    if clip_mount.is_none() {
        notes.push("no /data mount found; confirm where clips are written".into());
    }

    // The last hop: a host path is still not something we can open. Translate
    // it into our own mount, and be explicit when that isn't possible rather
    // than handing back a path that will fail later with a confusing error.
    let events_db_local_path = match (&sqlite_host_path, clip_mount.and_then(|m| m.source.as_deref()))
    {
        (Some(db_host), Some(backup_host)) => {
            match crate::setup::host_to_local(db_host, backup_host, local_backup_dir) {
                Some(p) => Some(p.to_string_lossy().to_string()),
                None => {
                    notes.push(format!(
                        "the database is at {db_host}, which is outside the directory mounted \
                         here as {} — mount the directory that contains it",
                        local_backup_dir.display()
                    ));
                    None
                }
            }
        }
        _ => None,
    };

    Ok(UpbInspection {
        running: state.running.unwrap_or(false),
        started_at: state.started_at,
        restart_count: raw.restart_count.unwrap_or(0),
        health_available: state.health.is_some(),
        retention: flag_value(&tokens, "--retention"),
        missing_range: flag_value(&tokens, "--missing-range"),
        proposed: ProposedConfig {
            backup_host_dir: clip_mount.and_then(|m| m.source.clone()),
            clip_path_prefix: clip_mount.map(|m| m.destination.clone()),
            sqlite_container_path,
            sqlite_host_path,
            events_db_local_path,
            notes,
        },
        container,
        mounts,
        env,
        env_withheld: withheld,
        command,
    })
}

/// Options for a follow-mode log stream, tailing `tail` lines first.
pub fn log_options(tail: &str) -> bollard::query_parameters::LogsOptions {
    LogsOptionsBuilder::default()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .timestamps(false)
        .tail(tail)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_flags_in_both_forms() {
        let split = t(&["/usr/bin/unifi-protect-backup", "--retention", "36500d"]);
        assert_eq!(flag_value(&split, "--retention").as_deref(), Some("36500d"));

        let joined = t(&["--retention=36500d"]);
        assert_eq!(flag_value(&joined, "--retention").as_deref(), Some("36500d"));

        assert_eq!(flag_value(&split, "--missing-range"), None);
    }

    #[test]
    fn flag_without_a_value_is_not_a_value() {
        assert_eq!(flag_value(&t(&["--retention"]), "--retention"), None);
    }

    #[test]
    fn tokenizes_a_command_that_arrived_as_one_string() {
        // How the reference compose's block-scalar `command:` can appear.
        let raw = t(&["/usr/bin/unifi-protect-backup --retention 36500d --missing-range 30d\n"]);
        let tokens = tokenize(&raw);
        assert_eq!(flag_value(&tokens, "--retention").as_deref(), Some("36500d"));
        assert_eq!(flag_value(&tokens, "--missing-range").as_deref(), Some("30d"));
    }

    #[test]
    fn resolves_container_paths_through_the_most_specific_mount() {
        let mounts = vec![
            MountInfo {
                source: Some("/srv/pool/protect/backup-service".into()),
                destination: "/config".into(),
                rw: true,
            },
            MountInfo {
                source: Some("/srv/pool/protect/backup-service".into()),
                destination: "/config/database".into(),
                rw: true,
            },
        ];
        assert_eq!(
            to_host_path(&mounts, "/config/database/events.sqlite").as_deref(),
            Some("/srv/pool/protect/backup-service/events.sqlite"),
        );
        // A path outside every mount is unreachable, not silently rewritten.
        assert_eq!(to_host_path(&mounts, "/elsewhere/events.sqlite"), None);
    }

    #[test]
    fn a_prefix_that_is_not_a_path_boundary_does_not_match() {
        let mounts = vec![MountInfo {
            source: Some("/host/data".into()),
            destination: "/data".into(),
            rw: false,
        }];
        assert_eq!(to_host_path(&mounts, "/database/events.sqlite"), None);
        assert_eq!(
            to_host_path(&mounts, "/data/cam/clip.mp4").as_deref(),
            Some("/host/data/cam/clip.mp4")
        );
    }
}
