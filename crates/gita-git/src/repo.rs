//! repo.rs
use anyhow::{anyhow, Context, Result};
use gix::Repository;
use std::path::PathBuf;
use std::process::Stdio;
use tracing::debug;

use crate::GitManager;
use crate::{find_cgi_header_end, git_cmd};
    /// Create a new manager rooted at `root`.
impl GitManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The filesystem root where bare repositories live.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The filesystem path of a repository with the given owner/name.
    pub fn repo_path(&self, owner: &str, name: &str) -> PathBuf {
        self.root.join(owner).join(format!("{name}.git"))
    }

    /// The filesystem path of a repository, rejecting any owner or name that
    /// could escape the repositories root.
    ///
    /// The owner segment is a directory name and the name segment becomes a
    /// `.git` directory, so both must be plain names: a value containing a
    /// path separator or `..` would let a caller create or delete a
    /// directory outside the root. Read paths use `repo_path` unchanged
    /// (a miss there is a miss, not a traversal), but anything that creates,
    /// moves or removes a repository goes through this check.
    pub fn safe_repo_path(&self, owner: &str, name: &str) -> Result<PathBuf> {
        for (label, value) in [("owner", owner), ("repository name", name)] {
            if value.is_empty() {
                return Err(anyhow!("{label} must not be empty"));
            }
            if value == "." || value == ".." {
                return Err(anyhow!("invalid {label}: {value}"));
            }
            if value.contains('/') || value.contains('\\') || value.contains('\0') {
                return Err(anyhow!("invalid {label}: {value}"));
            }
        }
        let path = self.repo_path(owner, name);
        // Belt and braces: confirm the resolved path is still under the root.
        // `Path::starts_with` compares components, so this also catches
        // anything the character checks above might have missed.
        if !path.starts_with(&self.root) {
            return Err(anyhow!(
                "repository path escapes the repositories root: {}",
                path.display()
            ));
        }
        Ok(path)
    }

    /// Initialize a new bare repository.
    pub async fn create_repo(
        &self,
        owner: &str,
        name: &str,
        default_branch: &str,
    ) -> Result<PathBuf> {
        let path = self.safe_repo_path(owner, name)?;
        if path.exists() {
            return Err(anyhow!("repository already exists: {owner}/{name}"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir {}", parent.display()))?;
        }
        let _repo =
            gix::init_bare(&path).map_err(|e| anyhow!("init bare repo {}: {e}", path.display()))?;
        // Set the default branch for fresh repositories.
        let _ = git_cmd()
            .arg("--git-dir")
            .arg(&path)
            .args([
                "symbolic-ref",
                "HEAD",
                &format!("refs/heads/{default_branch}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        debug!("created bare repo at {}", path.display());
        Ok(path)
    }

    /// Check whether a repository exists on disk.
    pub fn repo_exists(&self, owner: &str, name: &str) -> bool {
        self.repo_path(owner, name).exists()
    }

    /// Delete a repository directory (used when a repo is removed).
    pub fn delete_repo(&self, owner: &str, name: &str) -> Result<()> {
        let path = self.safe_repo_path(owner, name)?;
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("remove repo dir {}", path.display()))?;
        }
        Ok(())
    }

    /// Move a bare repository from one namespace/name to another.
    pub fn rename_repo(
        &self,
        old_owner: &str,
        old_name: &str,
        new_owner: &str,
        new_name: &str,
    ) -> Result<()> {
        let old_path = self.safe_repo_path(old_owner, old_name)?;
        let new_path = self.safe_repo_path(new_owner, new_name)?;
        if !old_path.exists() {
            return Err(anyhow!("repository not found: {old_owner}/{old_name}"));
        }
        if new_path.exists() {
            return Err(anyhow!(
                "target repository already exists: {new_owner}/{new_name}"
            ));
        }
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_path, &new_path)?;
        Ok(())
    }

    /// Fork: clone the bare repository at `(src_owner, src_name)` to a
    /// new location under `(dst_owner, dst_name)`. The destination must
    /// not exist. The clone uses `git clone --bare` so all objects and
    /// refs are copied without a working tree.
    pub fn fork_repo(
        &self,
        src_owner: &str,
        src_name: &str,
        dst_owner: &str,
        dst_name: &str,
    ) -> Result<()> {
        let src = self.safe_repo_path(src_owner, src_name)?;
        let dst = self.safe_repo_path(dst_owner, dst_name)?;
        if !src.exists() {
            return Err(anyhow!(
                "source repository not found: {src_owner}/{src_name}"
            ));
        }
        if dst.exists() {
            return Err(anyhow!(
                "destination repository already exists: {dst_owner}/{dst_name}"
            ));
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src_url = format!("file://{}", src.display());
        let output = git_cmd()
            .args(["clone", "--bare", "--no-local", &src_url])
            .arg(&dst)
            .output()
            .map_err(|e| anyhow!("fork git clone failed: {e}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "fork git clone error: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    /// Open a bare repository with gix.
    pub fn open(&self, owner: &str, name: &str) -> Result<Repository> {
        let path = self.repo_path(owner, name);
        gix::open(&path).with_context(|| format!("open bare repo {}", path.display()))
    }

    /// Run `git http-backend` as a CGI subprocess to serve a smart HTTP request.
    ///
    /// `GIT_HTTP_EXPORT_ALL=1` is always set, so the CGI will serve any
    /// repository under the root. That is a protocol-level decision with no
    /// per-repository granularity: without the variable `git http-backend`
    /// answers 404 for *every* repository (it has no per-repo export
    /// mechanism). The per-repository access control therefore has to live
    /// in the caller — see `gita-http::authorize_git_request`, which gates
    /// every request that addresses a repository, including the "dumb" file
    /// endpoints (`HEAD`, `objects/**`) that the CGI also serves.
    pub fn run_http_backend(
        &self,
        method: &str,
        path_info: &str,
        query_string: Option<&str>,
        content_type: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<(String, String)>, Vec<u8>)> {
        let mut cmd = git_cmd();
        cmd.arg("http-backend");
        cmd.env("GIT_PROJECT_ROOT", &self.root);
        cmd.env("GIT_HTTP_EXPORT_ALL", "1");
        cmd.env("PATH_INFO", path_info);
        cmd.env("REQUEST_METHOD", method);
        // Note: do not set GIT_PROTOCOL here. When it is set to
        // `version=2`, `git http-backend` serves receive-pack (push)
        // advertisements without the `# service=git-receive-pack`
        // preamble line, which git clients reject with
        // "not valid: is this a git repository?". Leaving it unset lets
        // http-backend negotiate the protocol version with the client.
        if let Some(qs) = query_string {
            cmd.env("QUERY_STRING", qs);
        }
        cmd.env("CONTENT_TYPE", content_type);
        cmd.env("CONTENT_LENGTH", body.len().to_string());
        // Enable the receive-pack (push) service. `git http-backend` only
        // serves receive-pack when the repository config sets
        // `http.receivepack = true`; inject that setting per-request so
        // push works for every repository without mutating on-disk configs.
        cmd.env("GIT_CONFIG_COUNT", "1");
        cmd.env("GIT_CONFIG_KEY_0", "http.receivepack");
        cmd.env("GIT_CONFIG_VALUE_0", "true");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("failed to spawn git http-backend: {e}"))?;

        // Write body to stdin.
        {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(body)?;
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| anyhow!("git http-backend failed: {e}"))?;

        if !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("http-backend stderr: {}", stderr.trim());
        }

        let stdout = output.stdout;
        let mut status: u16 = 200;
        let mut headers = Vec::new();
        let mut body: &[u8] = &stdout;

        // Parse status line and headers from the CGI output.
        if let Some(body_start) = find_cgi_header_end(&stdout) {
            let header_text = String::from_utf8_lossy(&stdout[..body_start]);
            for line in header_text.lines() {
                let line = line.trim_end_matches('\r');
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_string();
                    let value = value.trim().to_string();
                    if key.eq_ignore_ascii_case("Status") {
                        if let Some(code) = value.split_whitespace().next() {
                            if let Ok(c) = code.parse::<u16>() {
                                status = c;
                            }
                        }
                    } else {
                        headers.push((key, value));
                    }
                }
            }
            body = &stdout[body_start..];
        }

        Ok((status, headers, body.to_vec()))
    }
}
