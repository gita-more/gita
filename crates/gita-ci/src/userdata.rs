//! User data generation for CI jobs.
//!
//! The image template is a "half-finished" environment: what differs per
//! job (which commit to check out, which command to run, which SSH key to
//! use) is injected at boot time through cloud-init user data.
//!
//! The YAML produced here must be understood by cloud-init, so the keys
//! are the cloud-init ones (`write_files`, `runcmd`) — not camelCase or
//! kebab-case variants of them.

use anyhow::Result;
use serde::Serialize;
use serde_yaml;

#[derive(Serialize)]
struct UserData {
    /// cloud-config marker: cloud-init ignores the document without it.
    #[serde(rename = "cloud-config")]
    cloud_config: CloudConfigHeader,
    package_update: bool,
    package_upgrade: bool,
    write_files: Vec<WriteFile>,
    runcmd: Vec<String>,
    final_message: String,
}

/// cloud-init treats `cloud-config` as a plain string key.
type CloudConfigHeader = ();

#[derive(Serialize)]
struct WriteFile {
    path: String,
    content: String,
    permissions: String,
    owner: String,
}

/// Generate cloud-init user data for a CI job.
pub fn generate_userdata(
    repo_url: &str,
    commit: &str,
    ssh_private_key: &str,
    test_command: &str,
    database_url: &str,
) -> Result<String> {
    let user_data = UserData {
        cloud_config: (),
        package_update: false,
        package_upgrade: false,
        write_files: vec![WriteFile {
            path: "/home/ci/deploy.key".into(),
            content: ssh_private_key.into(),
            permissions: "0600".into(),
            owner: "ci:ci".into(),
        }],
        runcmd: vec![format!(
            r#"set -euo pipefail
pg_ctlcluster $(ls /etc/postgresql/ | head -1) main start || true
su - postgres -c "psql -c \"CREATE USER gita WITH PASSWORD 'gita';\"" || true
su - postgres -c "psql -c 'CREATE DATABASE gita OWNER gita;'" || true
export GIT_SSH_COMMAND="ssh -i /home/ci/deploy.key -o StrictHostKeyChecking=no"
if [ ! -d /home/ci/gita ]; then
  git clone {repo_url} /home/ci/gita
  cd /home/ci/gita
  git checkout {commit}
fi
export DATABASE_URL={database_url}
export GITA_TEST_DATABASE_URL={database_url}
cd /home/ci/gita
{test_command}"#
        )],
        final_message: "gita-ci-job complete".into(),
    };
    Ok(serde_yaml::to_string(&user_data)?)
}

/// Generate cloud-init user data for a one-shot VM job.
///
/// The VM boots, cloud-init writes the agent's environment and starts it.
/// The agent (already baked into the CI image) clones the repo, checks
/// out the commit, runs the script, and reports logs/status back to the
/// server with the per-job token. Nothing polls: one VM, one job.
///
/// The token is passed through the environment because it can be long —
/// well beyond what a systemd `ExecStart=` line comfortably carries.
pub fn generate_vm_job_userdata(
    server_url: &str,
    job_id: i64,
    agent_token: &str,
    repo_url: &str,
    commit: &str,
    script: &str,
    ssh_private_key: &str,
) -> Result<String> {
    // The script goes into its own file: job scripts are arbitrary
    // shell, and quoting one through an env-file line (or a shell
    // command) is a standing injection hazard. The agent receives the
    // file path instead of the text.
    let script_path = "/etc/gita-agent-script.sh";
    let env_path = "/etc/gita-agent.env";
    let mut write_files = vec![
        WriteFile {
            path: script_path.into(),
            content: script.into(),
            permissions: "0755".into(),
            owner: "root:root".into(),
        },
        WriteFile {
            path: env_path.into(),
            content: format!(
                "GITA_AGENT_SERVER_URL={server}\nGITA_AGENT_JOB_ID={job}\nGITA_AGENT_TOKEN={token}\nGITA_AGENT_REPO_URL={repo}\nGITA_AGENT_COMMIT={commit}\nGITA_AGENT_SCRIPT=bash {script_path}\n",
                server = server_url,
                job = job_id,
                token = agent_token,
                repo = repo_url,
                commit = commit,
                script_path = script_path,
            ),
            permissions: "0600".into(),
            owner: "root:root".into(),
        },
    ];
    if !ssh_private_key.is_empty() {
        write_files.push(WriteFile {
            path: "/home/ci/.ssh/id_ed25519".into(),
            content: ssh_private_key.into(),
            permissions: "0600".into(),
            owner: "ci:ci".into(),
        });
    }
    let user_data = UserData {
        cloud_config: (),
        package_update: false,
        package_upgrade: false,
        write_files,
        // The image ships the agent as a template unit keyed by job id
        // (`gita-ci-agent@.service`), which reads /etc/gita-agent.env.
        runcmd: vec![format!("systemctl start gita-ci-agent@{job}.service", job = job_id)],
        final_message: "gita vm job dispatched".into(),
    };
    Ok(serde_yaml::to_string(&user_data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_yaml() {
        let yaml = generate_userdata(
            "git@github.com:test/repo.git",
            "abc123",
            "-----BEGIN OPENSSH PRIVATE KEY-----\ntest\n-----END OPENSSH PRIVATE KEY-----",
            "cargo test --workspace",
            "postgres://gita:gita@localhost:55432/gita",
        ).unwrap();
        assert!(yaml.contains("write_files"));
        assert!(yaml.contains("runcmd"));
        assert!(yaml.contains("deploy.key"));
        assert!(yaml.contains("cargo test --workspace"));
    }

    /// cloud-init only treats a document as a cloud config when the
    /// `cloud-config` key is present, and it rejects the whole file
    /// otherwise — so the generated YAML must carry it.
    #[test]
    fn carries_the_cloud_config_header() {
        let yaml = generate_userdata(
            "git@github.com:test/repo.git",
            "abc123",
            "key",
            "cargo test",
            "postgres://gita:gita@localhost:55432/gita",
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(parsed.get("cloud-config").is_some());
        assert_eq!(
            parsed["write_files"][0]["permissions"].as_str(),
            Some("0600")
        );
        assert_eq!(parsed["write_files"][0]["owner"].as_str(), Some("ci:ci"));
    }

    /// The generated user data must be parseable as YAML and round-trip the
    /// job's parameters, so a broken template cannot silently reach a VM.
    #[test]
    fn embeds_the_job_parameters() {
        let yaml = generate_userdata(
            "git@github.com:test/repo.git",
            "deadbeef",
            "secret-key",
            "cargo test --workspace",
            "postgres://gita:gita@localhost:55432/gita",
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let runcmd = parsed["runcmd"][0].as_str().unwrap();
        assert!(runcmd.contains("git@github.com:test/repo.git"));
        assert!(runcmd.contains("deadbeef"));
        assert!(runcmd.contains("cargo test --workspace"));
        assert!(runcmd.contains("postgres://gita:gita@localhost:55432/gita"));
        assert_eq!(parsed["write_files"][0]["content"].as_str(), Some("secret-key"));
    }
}

#[cfg(test)]
mod vm_job_tests {
    use super::*;

    /// The VM-job user data must carry the agent's whole environment and
    /// start the agent unit.
    #[test]
    fn vm_job_userdata_embeds_agent_parameters() {
        let yaml = generate_vm_job_userdata(
            "http://gita.local:3000",
            42,
            "token-xyz",
            "http://gita.local:3000/owner/repo.git",
            "deadbeef",
            "cargo test --release",
            "",
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();

        assert!(parsed.get("cloud-config").is_some(), "must be a cloud config");

        let env_file = parsed["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "/etc/gita-agent.env")
            .expect("agent env file must be written");
        let content = env_file["content"].as_str().unwrap();
        assert!(content.contains("GITA_AGENT_SERVER_URL=http://gita.local:3000"));
        assert!(content.contains("GITA_AGENT_JOB_ID=42"));
        assert!(content.contains("GITA_AGENT_TOKEN=token-xyz"));
        assert!(content.contains("GITA_AGENT_COMMIT=deadbeef"));
        // The agent is pointed at the script file, not handed the text.
        assert!(content.contains("GITA_AGENT_SCRIPT=bash /etc/gita-agent-script.sh"));

        let script_file = parsed["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "/etc/gita-agent-script.sh")
            .expect("script file must be written");
        assert_eq!(script_file["content"].as_str(), Some("cargo test --release"));
        assert_eq!(script_file["permissions"].as_str(), Some("0755"));

        let runcmd = parsed["runcmd"][0].as_str().unwrap();
        assert!(runcmd.contains("systemctl start gita-ci-agent@42.service"));
    }

    /// A job script containing single quotes must survive the trip: it
    /// goes into a file verbatim, never through a shell quoting layer.
    #[test]
    fn vm_job_script_with_quotes_is_written_verbatim() {
        let script = "echo 'it\'s fine' && printf \"%s\\n\" done";
        let yaml = generate_vm_job_userdata("http://s", 1, "t", "http://r", "c", script, "").unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let script_file = parsed["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "/etc/gita-agent-script.sh")
            .unwrap();
        assert_eq!(script_file["content"].as_str(), Some(script));
    }

    /// An SSH key (private-repo cloning) is written with tight
    /// permissions; without one, no key file is written at all.
    #[test]
    fn vm_job_userdata_conditionally_includes_ssh_key() {
        let without = generate_vm_job_userdata("http://s", 1, "t", "http://r", "c", "x", "").unwrap();
        assert!(!without.contains("id_ed25519"));

        let with =
            generate_vm_job_userdata("http://s", 1, "t", "http://r", "c", "x", "SECRET-KEY")
                .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&with).unwrap();
        let key = parsed["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "/home/ci/.ssh/id_ed25519")
            .expect("key file must be written");
        assert_eq!(key["content"].as_str(), Some("SECRET-KEY"));
        assert_eq!(key["permissions"].as_str(), Some("0600"));
    }
}
