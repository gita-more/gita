#!/usr/bin/env rust
//! URL 路由测试：验证新的 GitLab/GitHub 风格 URL 结构
//!
//! 测试覆盖：
//! - /groups - 群组列表
//! - /:group - 群组详情
//! - /:group/repo - 群组仓库
//! - /:user/repo - 用户仓库
//! - /:group/subgroup - 子群组
//! - /:group/subgroup/repo - 子群组仓库
//! - 多层嵌套：/groupa/subgroupa/projectb

mod common;

use axum::http::{Method, StatusCode};
use common::{register_user, request, test_app};

/// 创建群组
async fn create_group(app: &gita_server::App, owner_token: &str, name: &str) {
    let (status, body) = request(
        app,
        Method::POST,
        "/api/groups",
        Some(serde_json::json!({"name": name, "description": ""})),
        Some(owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create group {name}: {body}");
}

/// 创建子群组
async fn create_subgroup(app: &gita_server::App, parent: &str, owner_token: &str, name: &str) {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/groups/{parent}/subgroups"),
        Some(serde_json::json!({"name": name, "description": ""})),
        Some(owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create subgroup {parent}/{name}: {body}");
}

/// 在群组下创建仓库
async fn create_group_repo(
    app: &gita_server::App,
    group: &str,
    token: &str,
    name: &str,
    is_private: bool,
) {
    let (status, body) = request(
        app,
        Method::POST,
        &format!("/api/groups/{group}/repos"),
        Some(serde_json::json!({
            "name": name,
            "description": "",
            "default_branch": "main",
            "is_private": is_private,
        })),
        Some(token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create group repo {group}/{name}: {body}"
    );
}

/// 获取群组详情
async fn get_group(app: &gita_server::App, path: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    request(app, Method::GET, &format!("/api/groups/{path}"), None, token).await
}

/// 获取仓库详情（新的 REST 路径）
async fn get_repo(app: &gita_server::App, owner: &str, name: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    request(app, Method::GET, &format!("/api/repos/{owner}/{name}"), None, token).await
}

/// 列出群组仓库
async fn list_group_repos(app: &gita_server::App, path: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    request(app, Method::GET, &format!("/api/groups/{path}/repos"), None, token).await
}

/// 列出子群组
async fn list_subgroups(app: &gita_server::App, path: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    request(app, Method::GET, &format!("/api/groups/{path}/subgroups"), None, token).await
}

/// 创建用户个人仓库
async fn create_user_repo(app: &gita_server::App, token: &str, name: &str, is_private: bool) {
    let (status, body) = request(
        app,
        Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": name,
            "description": "",
            "default_branch": "main",
            "is_private": is_private,
        })),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create user repo {name}: {body}");
}

#[tokio::test]
async fn group_list_endpoint() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "grouplist-owner").await;

    // 创建几个群组
    create_group(&app, &owner.token, "group-a").await;
    create_group(&app, &owner.token, "group-b").await;

    // 列出群组（需要认证）
    let (status, body) = request(
        &app,
        Method::GET,
        "/api/groups",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let groups = body.as_array().unwrap();
    assert!(groups.iter().any(|g| g["name"] == "group-a"));
    assert!(groups.iter().any(|g| g["name"] == "group-b"));
}

#[tokio::test]
async fn group_detail_by_full_path() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "detail-owner").await;

    // 创建根群组
    create_group(&app, &owner.token, "gita").await;

    // 通过 full_path 获取详情
    let (status, body) = get_group(&app, "gita", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["group"]["name"], "gita");
    assert_eq!(body["group"]["full_path"], "gita");
}

#[tokio::test]
async fn group_repo_via_rest_api() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "gr-owner").await;

    // 创建群组和仓库
    create_group(&app, &owner.token, "gita").await;
    create_group_repo(&app, "gita", &owner.token, "project-a", false).await;

    // 通过新的 REST 路径访问仓库
    let (status, body) = get_repo(&app, "gita", "project-a", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "gita");
    assert_eq!(body["name"], "project-a");
    assert_eq!(body["full_name"], "gita/project-a");
}

#[tokio::test]
async fn user_repo_via_rest_api() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "admin").await;

    // 创建用户个人仓库
    create_user_repo(&app, &owner.token, "my-project", false).await;

    // 通过 REST 路径访问
    let (status, body) = get_repo(&app, "admin", "my-project", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "admin");
    assert_eq!(body["name"], "my-project");
    assert_eq!(body["full_name"], "admin/my-project");
}

#[tokio::test]
async fn subgroup_creation_and_nesting() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "sub-owner").await;

    // 创建根群组
    create_group(&app, &owner.token, "groupa").await;

    // 创建子群组
    create_subgroup(&app, "groupa", &owner.token, "subgroupa").await;

    // 取父群组 id，供断言 parent_id 使用
    let (status, parent_body) = get_group(&app, "groupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    let parent_id = parent_body["group"]["id"].clone();

    // 获取子群组详情
    let (status, body) = get_group(&app, "groupa/subgroupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["group"]["name"], "subgroupa");
    assert_eq!(body["group"]["full_path"], "groupa/subgroupa");
    // parent_id should be groupa's id
    assert_eq!(body["group"]["parent_id"], parent_id);

    // 列出子群组
    let (status, body) = list_subgroups(&app, "groupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    let subgroups = body.as_array().unwrap();
    assert!(subgroups.iter().any(|g| g["name"] == "subgroupa" && g["full_path"] == "groupa/subgroupa"));
}

#[tokio::test]
async fn subgroup_repo_access() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "sr-owner").await;

    // 创建根群组 -> 子群组 -> 仓库
    create_group(&app, &owner.token, "groupa").await;
    create_subgroup(&app, "groupa", &owner.token, "subgroupa").await;
    create_group_repo(&app, "groupa/subgroupa", &owner.token, "project-b", false).await;

    // 通过 REST 路径访问子群组仓库
    let (status, body) = get_repo(&app, "groupa/subgroupa", "project-b", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "groupa/subgroupa");
    assert_eq!(body["name"], "project-b");
    assert_eq!(body["full_name"], "groupa/subgroupa/project-b");

    // 列出子群组仓库
    let (status, body) = list_group_repos(&app, "groupa/subgroupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    let repos = body.as_array().unwrap();
    assert!(repos.iter().any(|r| r["name"] == "project-b"));
}

#[tokio::test]
async fn three_level_nesting() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "deep-owner").await;

    // 三层嵌套: groupa/subgroupa/subsubgroup
    create_group(&app, &owner.token, "groupa").await;
    create_subgroup(&app, "groupa", &owner.token, "subgroupa").await;
    create_subgroup(&app, "groupa/subgroupa", &owner.token, "subsubgroup").await;

    // 验证三层路径
    let (status, body) = get_group(&app, "groupa/subgroupa/subsubgroup", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["group"]["full_path"], "groupa/subgroupa/subsubgroup");

    // 在最深层创建仓库
    create_group_repo(&app, "groupa/subgroupa/subsubgroup", &owner.token, "deep-project", false).await;

    // 访问深层仓库
    let (status, body) = get_repo(&app, "groupa/subgroupa/subsubgroup", "deep-project", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["full_name"], "groupa/subgroupa/subsubgroup/deep-project");
}

#[tokio::test]
async fn group_vs_user_namespace_collision() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "collision-owner").await;
    let _user = register_user(&app, "shared-name").await; // 用户名叫 shared-name

    // 创建群组，名字与用户名相同
    create_group(&app, &owner.token, "shared-name").await;
    create_group_repo(&app, "shared-name", &owner.token, "group-repo", false).await;

    // 同名用户创建个人仓库
    let shared_user = register_user(&app, "shared-name-2").await; // 另一个用户
    create_user_repo(&app, &shared_user.token, "user-repo", false).await;

    // 群组仓库
    let (status, body) = get_repo(&app, "shared-name", "group-repo", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "shared-name");

    // 用户仓库 (owner 是用户名)
    let (status, body) = get_repo(&app, "shared-name-2", "user-repo", Some(&shared_user.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "shared-name-2");
}

#[tokio::test]
async fn anonymous_group_detail_public_repos() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "anon-owner").await;

    create_group(&app, &owner.token, "public-group").await;
    create_group_repo(&app, "public-group", &owner.token, "public-repo", false).await;
    create_group_repo(&app, "public-group", &owner.token, "private-repo", true).await;

    // 匿名访问群组详情：只能看到公开仓库
    let (status, body) = get_group(&app, "public-group", None).await;
    assert_eq!(status, StatusCode::OK);
    let repos = body["repos"].as_array().unwrap();
    let names: Vec<&str> = repos.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"public-repo"));
    assert!(!names.contains(&"private-repo"), "匿名用户不应看到私有仓库");

    // 认证用户（群组所有者）能看到所有仓库
    let (status, body) = get_group(&app, "public-group", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    let repos = body["repos"].as_array().unwrap();
    let names: Vec<&str> = repos.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"public-repo"));
    assert!(names.contains(&"private-repo"));
}

#[tokio::test]
async fn rest_api_vs_git_http_paths() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "path-owner").await;

    create_group(&app, &owner.token, "mygroup").await;
    create_group_repo(&app, "mygroup", &owner.token, "myrepo", false).await;

    // REST API 路径: /api/repos/mygroup/myrepo
    let (status, body) = get_repo(&app, "mygroup", "myrepo", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "mygroup");

    // Git 克隆 URL 格式验证 (前端构建)
    // 预期: http://host/mygroup/myrepo.git（无 /git 前缀）
    // 这个由前端处理，后端只需保证仓库路径正确
    let (status, body) = get_group(&app, "mygroup", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["group"]["full_path"], "mygroup");
}

#[tokio::test]
async fn list_all_user_groups() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "list-owner").await;

    create_group(&app, &owner.token, "group1").await;
    create_group(&app, &owner.token, "group2").await;

    let (status, body) = request(
        &app,
        Method::GET,
        "/api/groups",
        None,
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let groups = body.as_array().unwrap();
    assert_eq!(groups.len(), 2);
    // 验证 full_path 字段存在
    for g in groups {
        assert!(g["full_path"].is_string());
        assert!(!g["full_path"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn subgroup_under_user_repo_not_confused() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "confuse-owner").await;

    // 用户创建个人仓库
    create_user_repo(&app, &owner.token, "myproject", false).await;

    // 用户名作为群组路径不应与个人仓库冲突
    let (status, body) = get_repo(&app, "confuse-owner", "myproject", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "confuse-owner"); // 个人仓库 owner 是用户名

    // 现在创建同名群组
    create_group(&app, &owner.token, "confuse-owner").await;
    create_group_repo(&app, "confuse-owner", &owner.token, "group-project", false).await;

    // 群组仓库
    let (status, body) = get_repo(&app, "confuse-owner", "group-project", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner"], "confuse-owner"); // 群组仓库 owner 是群组名
}

#[tokio::test]
async fn get_nonexistent_group_returns_404() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "404-owner").await;

    let (status, _) = get_group(&app, "nonexistent", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_nonexistent_repo_returns_404() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "404r-owner").await;

    let (status, _) = get_repo(&app, "nonexistent", "repo", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_group_with_parent_path() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "parent-owner").await;

    // 创建根群组
    create_group(&app, &owner.token, "root-group").await;

    // 取父群组 id，供断言 parent_id 使用
    let (status, parent_body) = get_group(&app, "root-group", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    let parent_id = parent_body["group"]["id"].clone();

    // 通过 API 创建子群组（使用 parent_path）
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/groups",
        Some(serde_json::json!({
            "name": "child-group",
            "description": "child of root",
            "parent_path": "root-group"
        })),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create child group via parent_path: {body}");
    assert_eq!(body["name"], "child-group");
    assert_eq!(body["full_path"], "root-group/child-group");
    // parent_id should be root-group's id
    assert_eq!(body["parent_id"], parent_id);
}

// ---------------------------------------------------------------------------
// /api/resolve — clean URL disambiguation
// ---------------------------------------------------------------------------

async fn resolve_path(
    app: &gita_server::App,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    request(app, Method::GET, &format!("/api/resolve/{path}"), None, token).await
}

#[tokio::test]
async fn resolve_personal_repo() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-personal").await;
    create_user_repo(&app, &owner.token, "myproject", false).await;

    let (status, body) = resolve_path(&app, "res-personal/myproject", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "resolve personal repo: {body}");
    assert_eq!(body["kind"], "repo");
    assert_eq!(body["owner"], "res-personal");
    assert_eq!(body["name"], "myproject");
    assert_eq!(body["owner_kind"], "user");
}

#[tokio::test]
async fn resolve_group_repo() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-group").await;
    create_group(&app, &owner.token, "gita").await;
    create_group_repo(&app, "gita", &owner.token, "project-a", false).await;

    let (status, body) = resolve_path(&app, "gita/project-a", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "resolve group repo: {body}");
    assert_eq!(body["kind"], "repo");
    assert_eq!(body["owner"], "gita");
    assert_eq!(body["name"], "project-a");
    assert_eq!(body["owner_kind"], "group");
}

#[tokio::test]
async fn resolve_group_and_subgroup_full_path() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-sub").await;
    create_group(&app, &owner.token, "groupa").await;
    create_subgroup(&app, "groupa", &owner.token, "subgroupa").await;

    // Top-level group.
    let (status, body) = resolve_path(&app, "groupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "resolve top-level group: {body}");
    assert_eq!(body["kind"], "group");
    assert_eq!(body["full_path"], "groupa");

    // Subgroup path — the case the SPA previously could not distinguish
    // from a repository URL by shape alone.
    let (status, body) = resolve_path(&app, "groupa/subgroupa", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "resolve subgroup path: {body}");
    assert_eq!(body["kind"], "group");
    assert_eq!(body["full_path"], "groupa/subgroupa");
}

#[tokio::test]
async fn resolve_nested_repo_three_segments() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-deep").await;
    create_group(&app, &owner.token, "groupa").await;
    create_subgroup(&app, "groupa", &owner.token, "subgroupa").await;
    create_group_repo(&app, "groupa/subgroupa", &owner.token, "project-b", false).await;

    let (status, body) = resolve_path(&app, "groupa/subgroupa/project-b", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "resolve nested repo: {body}");
    assert_eq!(body["kind"], "repo");
    assert_eq!(body["owner"], "groupa/subgroupa");
    assert_eq!(body["name"], "project-b");
    assert_eq!(body["owner_kind"], "group");
}

#[tokio::test]
async fn resolve_unknown_path_returns_404() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-404").await;

    let (status, _) = resolve_path(&app, "no-such-thing", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = resolve_path(&app, "no-such-thing/child", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolve_single_segment_username_returns_404() {
    let (app, _guard) = test_app().await;
    let _user = register_user(&app, "res-plain-user").await;

    // A bare username has no page: only groups are addressable by a
    // single-segment clean URL.
    let (status, _) = resolve_path(&app, "res-plain-user", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resolve_private_repo_hidden_from_anonymous() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "res-private").await;
    create_user_repo(&app, &owner.token, "secret", true).await;

    // Anonymous caller: the repository must be indistinguishable from a
    // non-existent one (no existence disclosure).
    let (status, _) = resolve_path(&app, "res-private/secret", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The owner resolves it normally.
    let (status, body) = resolve_path(&app, "res-private/secret", Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK, "owner must resolve own private repo: {body}");
    assert_eq!(body["kind"], "repo");
}

#[tokio::test]
async fn subgroup_cannot_shadow_repo_name_in_same_namespace() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "shadow-sub").await;
    create_group(&app, &owner.token, "groupa").await;
    create_group_repo(&app, "groupa", &owner.token, "clash", false).await;

    // A subgroup named like an existing repo in the same group would have
    // the same clean URL; the resolver ranks repositories first, so the
    // subgroup would be unreachable — refuse to create it.
    let (status, body) = create_subgroup_raw(&app, "groupa", &owner.token, "clash").await;
    assert_eq!(status, StatusCode::CONFLICT, "subgroup must not shadow repo: {body}");
}

#[tokio::test]
async fn repo_cannot_shadow_subgroup_name_in_same_namespace() {
    let (app, _guard) = test_app().await;
    let owner = register_user(&app, "shadow-repo").await;
    create_group(&app, &owner.token, "groupa").await;
    create_subgroup(&app, "groupa", &owner.token, "clash").await;

    // Mirror of the subgroup check: a repository must not take a child
    // subgroup's clean URL either.
    let (status, body) = request(
        &app,
        Method::POST,
        "/api/groups/groupa/repos",
        Some(serde_json::json!({"name": "clash", "description": "", "default_branch": "main"})),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "repo must not shadow subgroup: {body}");
}

/// create_subgroup that returns the raw status (the shared helper asserts OK).
async fn create_subgroup_raw(
    app: &gita_server::App,
    parent: &str,
    owner_token: &str,
    name: &str,
) -> (StatusCode, serde_json::Value) {
    request(
        app,
        Method::POST,
        &format!("/api/groups/{parent}/subgroups"),
        Some(serde_json::json!({"name": name, "description": ""})),
        Some(owner_token),
    )
    .await
}