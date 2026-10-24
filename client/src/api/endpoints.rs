//! Typed wrappers for every REST endpoint.
//!
//! Each method calls `GitaClient::request()` with the right method/path/body
//! and returns a typed DTO from `types.rs`. Paths mirror
//! `docs/api.md` and the route registrations in `crates/gita-api/src/routes`.

use anyhow::Result;
use serde_json::json;
use url::form_urlencoded;

use super::GitaClient;
use crate::api::types::*;

impl GitaClient {
    /// Issue an arbitrary authenticated request and return the raw JSON body.
    pub async fn api(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| anyhow::anyhow!("invalid HTTP method: {method}"))?;
        self.request(method, path, body.as_ref()).await
    }

    // ------------------------------------------------------------------
    // auth
    // ------------------------------------------------------------------

    pub async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthResponse> {
        self.request(
            reqwest::Method::POST,
            "/api/auth/register",
            Some(&json!({ "username": username, "email": email, "password": password })),
        )
        .await
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<AuthResponse> {
        self.request(
            reqwest::Method::POST,
            "/api/auth/login",
            Some(&json!({ "username": username, "password": password })),
        )
        .await
    }

    pub async fn me(&self) -> Result<User> {
        self.request(reqwest::Method::GET, "/api/auth/me", None)
            .await
    }

    pub async fn update_me(
        &self,
        locale: Option<&str>,
        email: Option<&str>,
        current_password: Option<&str>,
        new_password: Option<&str>,
    ) -> Result<User> {
        self.request(
            reqwest::Method::PATCH,
            "/api/auth/me",
            Some(&json!({
                "locale": locale,
                "email": email,
                "current_password": current_password,
                "new_password": new_password,
            })),
        )
        .await
    }

    pub async fn logout(&self) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::POST,
            "/api/auth/logout",
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn create_token(&self, name: Option<&str>) -> Result<Token> {
        self.request(
            reqwest::Method::POST,
            "/api/auth/tokens",
            Some(&json!({ "name": name })),
        )
        .await
    }

    pub async fn list_tokens(&self) -> Result<Vec<Token>> {
        self.request(reqwest::Method::GET, "/api/auth/tokens", None)
            .await
    }

    pub async fn delete_token(&self, id: i64) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/auth/tokens/{id}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn list_ssh_keys(&self) -> Result<SshKeysResponse> {
        self.request(reqwest::Method::GET, "/api/auth/keys", None)
            .await
    }

    pub async fn add_ssh_key(&self, key: &str, title: Option<&str>) -> Result<SshKey> {
        self.request(
            reqwest::Method::POST,
            "/api/auth/keys",
            // The server expects the OpenSSH line in the `public_key` field.
            Some(&json!({ "public_key": key, "title": title })),
        )
        .await
    }

    pub async fn delete_ssh_key(&self, id: i64) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/auth/keys/{id}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ------------------------------------------------------------------
    // repos
    // ------------------------------------------------------------------

    pub async fn list_repos(&self, owner: Option<&str>) -> Result<Vec<Repository>> {
        let mut path = "/api/repos".to_string();
        if let Some(o) = owner {
            path.push_str(&format!(
                "?owner={}",
                form_urlencoded::byte_serialize(o.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_repo(&self, owner: &str, repo: &str) -> Result<Repository> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}"),
            None,
        )
        .await
    }

    pub async fn create_repo(
        &self,
        name: &str,
        description: Option<&str>,
        is_private: Option<bool>,
        default_branch: Option<&str>,
    ) -> Result<Repository> {
        self.request(
            reqwest::Method::POST,
            "/api/repos",
            Some(&json!({
                "name": name,
                "description": description,
                "is_private": is_private,
                "default_branch": default_branch,
            })),
        )
        .await
    }

    pub async fn delete_repo(&self, owner: &str, repo: &str) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/repos/{owner}/{repo}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn update_repo(
        &self,
        owner: &str,
        repo: &str,
        description: Option<&str>,
        is_private: Option<bool>,
        new_name: Option<&str>,
        new_owner: Option<&str>,
    ) -> Result<Repository> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/api/repos/{owner}/{repo}"),
            Some(&json!({
                "description": description,
                "is_private": is_private,
                "new_name": new_name,
                "new_owner": new_owner,
            })),
        )
        .await
    }

    pub async fn get_tree(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        r#ref: &str,
    ) -> Result<TreeResponse> {
        let path = format!(
            "/api/repos/{owner}/{repo}/tree?path={}&ref={}",
            form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>(),
            form_urlencoded::byte_serialize(r#ref.as_bytes()).collect::<String>()
        );
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_file(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        r#ref: &str,
    ) -> Result<FileContent> {
        let path = format!(
            "/api/repos/{owner}/{repo}/file?path={}&ref={}",
            form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>(),
            form_urlencoded::byte_serialize(r#ref.as_bytes()).collect::<String>()
        );
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_commits(
        &self,
        owner: &str,
        repo: &str,
        r#ref: Option<&str>,
        limit: usize,
    ) -> Result<CommitsResponse> {
        let mut path = format!("/api/repos/{owner}/{repo}/commits?limit={limit}");
        if let Some(r) = r#ref {
            path.push_str(&format!(
                "&ref={}",
                form_urlencoded::byte_serialize(r.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_commit(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/commits/{sha}"),
            None,
        )
        .await
    }

    pub async fn get_branches(&self, owner: &str, repo: &str) -> Result<Vec<String>> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/branches"),
            None,
        )
        .await
    }

    pub async fn create_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        r#ref: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/branches"),
            Some(&json!({ "name": branch, "ref": r#ref })),
        )
        .await
    }

    pub async fn delete_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/repos/{owner}/{repo}/branches/{branch}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn get_tags(&self, owner: &str, repo: &str) -> Result<Vec<String>> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/tags"),
            None,
        )
        .await
    }

    pub async fn create_tag(
        &self,
        owner: &str,
        repo: &str,
        tag: &str,
        r#ref: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/tags"),
            Some(&json!({ "name": tag, "ref": r#ref })),
        )
        .await
    }

    pub async fn delete_tag(&self, owner: &str, repo: &str, tag: &str) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/repos/{owner}/{repo}/tags/{tag}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn star_repo(&self, owner: &str, repo: &str) -> Result<StarResponse> {
        self.request(
            reqwest::Method::PUT,
            &format!("/api/repos/{owner}/{repo}/star"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn unstar_repo(&self, owner: &str, repo: &str) -> Result<StarResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/repos/{owner}/{repo}/star"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn get_star(&self, owner: &str, repo: &str) -> Result<StarResponse> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/star"),
            None,
        )
        .await
    }

    pub async fn fork_repo(
        &self,
        owner: &str,
        repo: &str,
        name: Option<&str>,
    ) -> Result<Repository> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/fork"),
            Some(&json!({ "name": name })),
        )
        .await
    }

    // ------------------------------------------------------------------
    // pull requests
    // ------------------------------------------------------------------

    pub async fn list_prs(
        &self,
        owner: &str,
        repo: &str,
        status: Option<&str>,
    ) -> Result<PrsResponse> {
        let mut path = format!("/api/repos/{owner}/{repo}/prs");
        if let Some(s) = status {
            path.push_str(&format!(
                "?status={}",
                form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_pr(&self, owner: &str, repo: &str, number: i64) -> Result<PullRequest> {
        let v: serde_json::Value = self
            .request(
                reqwest::Method::GET,
                &format!("/api/repos/{owner}/{repo}/prs/{number}"),
                None,
            )
            .await?;
        Ok(serde_json::from_value(v["pr"].clone())?)
    }

    pub async fn create_pr(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: Option<&str>,
        source_branch: &str,
        target_branch: Option<&str>,
    ) -> Result<PullRequest> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/prs"),
            Some(&json!({
                "title": title,
                "body": body,
                "source_branch": source_branch,
                "target_branch": target_branch,
            })),
        )
        .await
    }

    pub async fn merge_pr(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/merge"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn close_pr(&self, owner: &str, repo: &str, number: i64) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/close"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/diff"),
            None,
        )
        .await
    }

    pub async fn get_pr_files(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/files"),
            None,
        )
        .await
    }

    pub async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<CommentsResponse> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/comments"),
            None,
        )
        .await
    }

    pub async fn create_pr_comment(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        body: &str,
    ) -> Result<Comment> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/prs/{number}/comments"),
            Some(&json!({ "body": body })),
        )
        .await
    }

    // ------------------------------------------------------------------
    // issues
    // ------------------------------------------------------------------

    pub async fn list_issues(
        &self,
        owner: &str,
        repo: &str,
        status: Option<&str>,
    ) -> Result<IssuesResponse> {
        let mut path = format!("/api/repos/{owner}/{repo}/issues");
        if let Some(s) = status {
            path.push_str(&format!(
                "?status={}",
                form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn get_issue(&self, owner: &str, repo: &str, number: i64) -> Result<Issue> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/issues/{number}"),
            None,
        )
        .await
    }

    pub async fn list_all_issues(&self, status: Option<&str>) -> Result<GlobalIssuesResponse> {
        let mut path = "/api/issues".to_string();
        if let Some(s) = status {
            path.push_str(&format!(
                "?status={}",
                form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: Option<&str>,
    ) -> Result<Issue> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/issues"),
            Some(&json!({ "title": title, "body": body })),
        )
        .await
    }

    pub async fn close_issue(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/issues/{number}/close"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn list_issue_comments(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> Result<CommentsResponse> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/issues/{number}/comments"),
            None,
        )
        .await
    }

    pub async fn create_issue_comment(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        body: &str,
    ) -> Result<Comment> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/issues/{number}/comments"),
            Some(&json!({ "body": body })),
        )
        .await
    }

    pub async fn delete_comment(&self, id: i64) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/comments/{id}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ------------------------------------------------------------------
    // groups
    // ------------------------------------------------------------------

    pub async fn list_groups(&self) -> Result<Vec<Group>> {
        self.request(reqwest::Method::GET, "/api/groups", None)
            .await
    }

    pub async fn create_group(&self, name: &str, description: Option<&str>) -> Result<Group> {
        self.request(
            reqwest::Method::POST,
            "/api/groups",
            Some(&json!({ "name": name, "description": description })),
        )
        .await
    }

    pub async fn get_group(&self, name: &str) -> Result<GroupView> {
        self.request(reqwest::Method::GET, &format!("/api/groups/{name}"), None)
            .await
    }

    pub async fn create_group_repo(
        &self,
        group: &str,
        name: &str,
        description: Option<&str>,
        is_private: Option<bool>,
        default_branch: Option<&str>,
    ) -> Result<Repository> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/groups/{group}/repos"),
            Some(&json!({
                "name": name,
                "description": description,
                "is_private": is_private,
                "default_branch": default_branch,
            })),
        )
        .await
    }

    pub async fn list_group_members(&self, name: &str) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/groups/{name}/members"),
            None,
        )
        .await
    }

    pub async fn add_group_member(
        &self,
        group: &str,
        username: &str,
        role: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/groups/{group}/members"),
            Some(&json!({ "username": username, "role": role })),
        )
        .await
    }

    pub async fn update_group_member(
        &self,
        group: &str,
        username: &str,
        role: &str,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/api/groups/{group}/members/{username}"),
            Some(&json!({ "role": role })),
        )
        .await
    }

    pub async fn remove_group_member(&self, group: &str, username: &str) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/groups/{group}/members/{username}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ------------------------------------------------------------------
    // search
    // ------------------------------------------------------------------

    pub async fn search_repos(&self, q: &str) -> Result<HitsResponse<RepoSearchHit>> {
        let path = format!(
            "/api/search/repos?q={}",
            form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
        );
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn search_code(&self, q: &str) -> Result<HitsResponse<CodeSearchHit>> {
        let path = format!(
            "/api/search/code?q={}",
            form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
        );
        self.request(reqwest::Method::GET, &path, None).await
    }

    // ------------------------------------------------------------------
    // events
    // ------------------------------------------------------------------

    pub async fn list_events(
        &self,
        limit: usize,
        repo: Option<&str>,
        event_type: Option<&str>,
    ) -> Result<EventsResponse> {
        let mut path = format!("/api/events?limit={limit}");
        if let Some(r) = repo {
            path.push_str(&format!(
                "&repo={}",
                form_urlencoded::byte_serialize(r.as_bytes()).collect::<String>()
            ));
        }
        if let Some(t) = event_type {
            path.push_str(&format!(
                "&type={}",
                form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>()
            ));
        }
        self.request(reqwest::Method::GET, &path, None).await
    }

    // ------------------------------------------------------------------
    // pipelines
    // ------------------------------------------------------------------

    pub async fn list_pipelines(&self, owner: &str, repo: &str) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/pipelines"),
            None,
        )
        .await
    }

    pub async fn list_runs(&self, owner: &str, repo: &str, path: &str) -> Result<RunsResponse> {
        let path = format!("/api/repos/{owner}/{repo}/pipelines/{}/runs", path);
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn trigger_run(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/pipelines/{}/runs", path),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn get_run(&self, owner: &str, repo: &str, run_id: i64) -> Result<RunDetailResponse> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/runs/{run_id}"),
            None,
        )
        .await
    }

    pub async fn get_job_logs(
        &self,
        owner: &str,
        repo: &str,
        run_id: i64,
        job_id: i64,
        since: i64,
    ) -> Result<LogsResponse> {
        let path =
            format!("/api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs?since={since}");
        self.request(reqwest::Method::GET, &path, None).await
    }

    pub async fn cancel_run(
        &self,
        owner: &str,
        repo: &str,
        run_id: i64,
    ) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::POST,
            &format!("/api/repos/{owner}/{repo}/runs/{run_id}/cancel"),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ------------------------------------------------------------------
    // pages
    // ------------------------------------------------------------------

    pub async fn get_pages_status(&self, owner: &str, repo: &str) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/pages/status"),
            None,
        )
        .await
    }

    pub async fn get_pages(&self, owner: &str, repo: &str) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/api/repos/{owner}/{repo}/pages"),
            None,
        )
        .await
    }

    pub async fn put_pages(
        &self,
        owner: &str,
        repo: &str,
        enabled: bool,
        branch: &str,
        path: &str,
    ) -> Result<serde_json::Value> {
        self.request(
            reqwest::Method::PUT,
            &format!("/api/repos/{owner}/{repo}/pages"),
            Some(&json!({ "enabled": enabled, "branch": branch, "path": path })),
        )
        .await
    }

    pub async fn delete_pages(&self, owner: &str, repo: &str) -> Result<MessageResponse> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/api/repos/{owner}/{repo}/pages"),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ------------------------------------------------------------------
    // health
    // ------------------------------------------------------------------

    pub async fn health(&self) -> Result<serde_json::Value> {
        self.request(reqwest::Method::GET, "/api/health", None)
            .await
    }
}
