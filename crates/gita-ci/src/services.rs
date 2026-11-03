//! Service containers for a CI job.
//!
//! This mirrors what the GitHub Actions runner does in
//! `ContainerOperationProvider.cs`:
//!
//! 1. create one Docker bridge network for the job, so two jobs on the same
//!    host never collide on a port;
//! 2. start one container per `services:` entry, attached to that network,
//!    with the service's label as its network alias (that is why a step can
//!    reach `postgres:5432` by name when the job itself runs in a
//!    container);
//! 3. publish the declared `HOST:CONTAINER` ports, which is how a step that
//!    runs directly on the VM reaches the service on `localhost`;
//! 4. pass `options:` through to `docker create` unchanged — that is where
//!    `--health-cmd` lives;
//! 5. wait for `docker inspect .State.Health.Status` to report `healthy`.
//!
//! Job scripts run directly on the host (there is no `container:` support
//! yet), so services are reached through published ports on `localhost`,
//! exactly as on a GitHub-hosted runner.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gita_workflow::{Container, Service};
use tokio::process::Command;

/// Where the job's workspace is mounted inside a `container:` job.
pub const JOB_CONTAINER_WORKDIR: &str = "/workspace";

/// How long to wait for one service to report healthy.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);

/// The Docker network created for one job.
pub fn network_name(job_id: i64) -> String {
    format!("gita_job_network_{job_id}")
}

/// The container name for one service of one job.
pub fn container_name(job_id: i64, label: &str) -> String {
    format!("gita-svc-{job_id}-{label}")
}

/// The `docker create` argument list for one service.
///
/// Kept separate from running it so the exact command line can be asserted
/// in a unit test without a Docker daemon.
pub fn create_args(job_id: i64, network: &str, label: &str, svc: &Service) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        "--name".into(),
        container_name(job_id, label),
        "--network".into(),
        network.to_string(),
        // The label is the hostname other containers on the network use.
        "--network-alias".into(),
        label.to_string(),
    ];

    for port in &svc.ports {
        args.push("-p".into());
        args.push(port.clone());
    }
    let mut env: Vec<(&String, &String)> = svc.env.iter().collect();
    env.sort();
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    for volume in &svc.volumes {
        args.push("-v".into());
        args.push(volume.clone());
    }
    if let Some(entrypoint) = &svc.entrypoint {
        args.push("--entrypoint".into());
        args.push(entrypoint.clone());
    }
    // `options:` is a raw docker CLI fragment (`--health-cmd "..."`), so it
    // is appended as separate words rather than one argument.
    if let Some(options) = &svc.options {
        args.extend(options.split_whitespace().map(str::to_string));
    }
    args.push(svc.image.clone());
    if let Some(command) = &svc.command {
        args.extend(command.split_whitespace().map(str::to_string));
    }
    args
}

/// What happened to one service.
#[derive(Debug, Default)]
pub struct Services {
    network: String,
    /// Container names, in start order, for teardown.
    started: Vec<String>,
}

impl Services {
    /// Create the job's network and start every declared service.
    ///
    /// A service whose `image` is empty is skipped (GitHub's
    /// conditional-service idiom). A failure to start or a service that
    /// never becomes healthy is an error: the job must not run against a
    /// half-built environment.
    pub async fn start(
        job_id: i64,
        services: &BTreeMap<String, Service>,
        log: &(dyn Fn(&str) + Sync),
    ) -> anyhow::Result<Self> {
        // An empty map, or one whose every image is empty, needs no
        // network. `all` alone is vacuously true on an empty map, which is
        // why the emptiness is checked too: otherwise every job without
        // services would still try to tear a network down at the end.
        if services.values().all(|s| s.image.is_empty()) {
            return Ok(Self {
                network: String::new(),
                started: Vec::new(),
            });
        }
        let mut this = Services {
            network: network_name(job_id),
            started: Vec::new(),
        };

        log(&format!("creating docker network {}", this.network));
        run_docker(&["network", "create", &this.network]).await?;

        for (label, svc) in services {
            if svc.image.is_empty() {
                log(&format!("service '{label}' has an empty image; skipping"));
                continue;
            }
            log(&format!("starting service '{label}' ({})", svc.image));
            let args = create_args(job_id, &this.network, label, svc);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            if let Err(e) = run_docker(&refs).await {
                log(&format!("service '{label}' failed to create: {e}"));
                this.stop().await;
                return Err(e);
            }
            let name = container_name(job_id, label);
            this.started.push(name.clone());
            if let Err(e) = run_docker(&["start", &name]).await {
                log(&format!("service '{label}' failed to start: {e}"));
                this.stop().await;
                return Err(e);
            }
            if let Err(e) = wait_healthy(&name, HEALTH_TIMEOUT).await {
                log(&format!("service '{label}' did not become healthy: {e}"));
                log(&format!("--- logs of {name} ---"));
                if let Ok(logs) = docker_output(&["logs", &name]).await {
                    for line in logs.lines() {
                        log(line);
                    }
                }
                this.stop().await;
                return Err(e);
            }
            log(&format!("service '{label}' is healthy"));
        }
        Ok(this)
    }

    /// Remove the containers and the network. Best-effort: teardown failure
    /// must not mask the job's real result, and the VM is reclaimed anyway.
    pub async fn stop(&mut self) {
        for name in self.started.drain(..) {
            let _ = run_docker(&["rm", "-f", &name]).await;
        }
        if !self.network.is_empty() {
            let _ = run_docker(&["network", "rm", &self.network]).await;
        }
    }
}

async fn run_docker(args: &[&str]) -> anyhow::Result<()> {
    let output = docker_output(args).await?;
    let _ = output;
    Ok(())
}

async fn docker_output(args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("docker").args(args).output().await?;
    if !out.status.success() {
        anyhow::bail!(
            "docker {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The `docker run` argument list for a `container:` job.
///
/// Separate from running it so the command line can be asserted without a
/// Docker daemon. The workspace is bind-mounted at
/// [`JOB_CONTAINER_WORKDIR`] and the job's script is the `bash -ec` that
/// runs there.
pub fn container_run_args(
    name: &str,
    network: &str,
    container: &Container,
    work_dir: &Path,
    script: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--name".into(),
        name.to_string(),
        "--network".into(),
        network.to_string(),
        "-v".into(),
        format!("{}:{}", work_dir.display(), JOB_CONTAINER_WORKDIR),
        "-w".into(),
        JOB_CONTAINER_WORKDIR.to_string(),
    ];
    for port in &container.ports {
        args.push("-p".into());
        args.push(port.clone());
    }
    let mut env: Vec<(&String, &String)> = container.env.iter().collect();
    env.sort();
    for (k, v) in env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    for volume in &container.volumes {
        args.push("-v".into());
        args.push(volume.clone());
    }
    if let Some(options) = &container.options {
        args.extend(options.split_whitespace().map(str::to_string));
    }
    args.push(container.image.clone());
    args.push("bash".into());
    args.push("-ec".into());
    args.push(script.to_string());
    args
}

/// Run a job's script inside a Docker container (`container:`).
///
/// GitHub runs the job's steps *in* the container instead of on the
/// runner. The container joins the same per-job network as the services,
/// so a step reaches `postgres:5432` by name and needs no published port.
///
/// The workspace is bind-mounted at [`JOB_CONTAINER_WORKDIR`]; the runner
/// execs `bash -ec <script>` in it, exactly as it would on the host. The
/// container is force-removed afterwards, services and network included.
pub async fn run_in_container(
    job_id: i64,
    container: &Container,
    services: &BTreeMap<String, Service>,
    work_dir: &Path,
    script: &str,
    log: &(dyn Fn(&str) + Sync),
) -> anyhow::Result<i32> {
    let mut this = Services {
        network: network_name(job_id),
        started: Vec::new(),
    };
    log(&format!("creating docker network {}", this.network));
    run_docker(&["network", "create", &this.network]).await?;

    let result = run_in_container_inner(
        job_id,
        container,
        services,
        work_dir,
        script,
        log,
        &mut this,
    )
    .await;
    this.stop().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_in_container_inner(
    job_id: i64,
    container: &Container,
    services: &BTreeMap<String, Service>,
    work_dir: &Path,
    script: &str,
    log: &(dyn Fn(&str) + Sync),
    this: &mut Services,
) -> anyhow::Result<i32> {
    // Services first: a step must find them ready on its first line.
    for (label, svc) in services {
        if svc.image.is_empty() {
            log(&format!("service '{label}' has an empty image; skipping"));
            continue;
        }
        log(&format!("starting service '{label}' ({})", svc.image));
        let args = create_args(job_id, &this.network, label, svc);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_docker(&refs).await?;
        let name = container_name(job_id, label);
        this.started.push(name.clone());
        run_docker(&["start", &name]).await?;
        wait_healthy(&name, HEALTH_TIMEOUT).await?;
        log(&format!("service '{label}' is healthy"));
    }

    // The job container.
    let name = container_name(job_id, "job");
    let args = container_run_args(&name, &this.network, container, work_dir, script);
    this.started.push(name.clone());
    log(&format!(
        "running the job in container {name} ({})",
        container.image
    ));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let status = Command::new("docker").args(&refs).status().await?;
    // A signal-killed container has no code; report it as a failure rather
    // than pretending it succeeded.
    Ok(status.code().unwrap_or(1))
}

/// Poll `docker inspect` until the container reports `healthy`.
///
/// A container with no `HEALTHCHECK` reports an empty status; that counts
/// as healthy, matching the runner's behaviour.
async fn wait_healthy(name: &str, timeout: Duration) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = Duration::from_secs(2);
    loop {
        let status = docker_output(&[
            "inspect",
            "--format",
            "{{if .State.Health}}{{.State.Health.Status}}{{end}}",
            name,
        ])
        .await?;
        let status = status.trim();
        if status.is_empty() || status == "healthy" {
            return Ok(());
        }
        if status == "unhealthy" {
            anyhow::bail!("container {name} is unhealthy");
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("container {name} still '{status}' after {timeout:?}");
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(32));
    }
}

/// Share one `Services` handle between the job and a teardown path.
pub type SharedServices = Arc<tokio::sync::Mutex<Services>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> Service {
        Service {
            image: "postgres:17".into(),
            env: BTreeMap::from([("POSTGRES_DB".to_string(), "gita".to_string())]),
            ports: vec!["55432:5432".into()],
            options: Some("--health-cmd \"pg_isready\"".into()),
            ..Default::default()
        }
    }

    /// The command line must carry the network and the alias, the
    /// `HOST:CONTAINER` port, env, the raw options and the image last.
    #[test]
    fn create_args_match_docker_cli() {
        let args = create_args(7, "gita_job_network_7", "postgres", &svc());
        let joined = args.join(" ");
        assert_eq!(args[0], "create");
        assert!(joined.contains("--name gita-svc-7-postgres"));
        assert!(joined.contains("--network gita_job_network_7"));
        assert!(joined.contains("--network-alias postgres"));
        assert!(joined.contains("-p 55432:5432"));
        assert!(joined.contains("-e POSTGRES_DB=gita"));
        assert!(joined.contains("--health-cmd"));
        // The image comes after every flag and before the command.
        assert_eq!(args.last().unwrap(), "postgres:17");
    }

    /// `command:` overrides the image CMD, so it goes after the image.
    #[test]
    fn command_follows_the_image() {
        let mut s = svc();
        s.command = Some("postgres -c fsync=off".into());
        let args = create_args(1, "n", "db", &s);
        let pos = args.iter().position(|a| a == "postgres:17").unwrap();
        assert_eq!(&args[pos + 1..], &["postgres", "-c", "fsync=off"]);
    }

    /// A job with no services must not touch docker at all — not even to
    /// remove a network it never created.
    #[tokio::test]
    async fn no_services_means_no_network() {
        let log = |_: &str| {};
        let started = Services::start(1, &BTreeMap::new(), &log).await.unwrap();
        assert!(started.started.is_empty());
        assert!(
            started.network.is_empty(),
            "an empty job must not claim a network name"
        );
    }

    /// A `container:` job runs `bash -ec <script>` in the image, with the
    /// workspace mounted and the container on the job network.
    #[test]
    fn container_run_args_match_docker_cli() {
        let container = Container {
            image: "node:20".into(),
            env: BTreeMap::from([("NODE_ENV".to_string(), "test".to_string())]),
            ports: vec!["8080:80".into()],
            options: Some("--memory 2g".into()),
            ..Default::default()
        };
        let args = container_run_args(
            "gita-svc-7-job",
            "gita_job_network_7",
            &container,
            Path::new("/tmp/work"),
            "npm test",
        );
        let joined = args.join(" ");
        assert_eq!(args[0], "run");
        assert!(joined.contains("--name gita-svc-7-job"));
        assert!(joined.contains("--network gita_job_network_7"));
        // The checkout is visible inside at a fixed path, and that path is
        // the working directory.
        assert!(joined.contains("-v /tmp/work:/workspace"));
        assert!(joined.contains("-w /workspace"));
        assert!(joined.contains("-p 8080:80"));
        assert!(joined.contains("-e NODE_ENV=test"));
        assert!(joined.contains("--memory 2g"));

        // The image comes right before `bash -ec <script>`.
        let image_at = args.iter().position(|a| a == "node:20").unwrap();
        assert_eq!(&args[image_at + 1..], &["bash", "-ec", "npm test"]);
    }

    /// A service with no image is the conditional-service idiom and must
    /// not produce a container.
    #[tokio::test]
    async fn empty_image_is_skipped_without_touching_docker() {
        let services = BTreeMap::from([("db".to_string(), Service::default())]);
        let lines = std::sync::Mutex::new(Vec::new());
        let log = |l: &str| lines.lock().unwrap().push(l.to_string());
        let started = Services::start(1, &services, &log).await.unwrap();
        assert!(started.started.is_empty());
        assert!(lines.lock().unwrap().is_empty());
    }
}
