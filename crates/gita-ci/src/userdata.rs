//! User data generation for CI jobs.

use anyhow::Result;
use serde::Serialize;
use serde_yaml;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct UserData {
    write_files: Vec<WriteFile>,
    runcmd: Vec<String>,
}

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
        write_files: vec![WriteFile {
            path: "/home/ci/deploy.key".into(),
            content: ssh_private_key.into(),
            permissions: "0600".into(),
            owner: "ci:ci".into(),
        }],
        runcmd: vec![
            format!("export GIT_SSH_COMMAND=\"ssh -i /home/ci/deploy.key -o StrictHostKeyChecking=no\""),
            format!("git clone {} /home/ci/gita", repo_url),
            format!("cd /home/ci/gita && git checkout {}", commit),
            format!("export DATABASE_URL={} && export GITA_TEST_DATABASE_URL={} && cd /home/ci/gita && {}", database_url, database_url, test_command),
        ],
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
}