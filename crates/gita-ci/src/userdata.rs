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
