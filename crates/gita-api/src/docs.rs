//! OpenAPI documentation.
//!
//! Serves a hand-written OpenAPI 3.0 JSON document at
//! `/api/docs/openapi.json` and a Swagger UI HTML page at `/api/docs`.
//! The UI loads the standard Swagger UI assets from a CDN.

use crate::AppState;

/// The OpenAPI 3.0 document describing Gita's REST API.
///
/// Kept as a static string to avoid a build-time dependency on Swagger
/// UI assets. Core endpoints are documented; the schema section lists
/// the main DTOs.
pub const OPENAPI_JSON: &str = r##"{
  "openapi": "3.0.3",
  "info": {
    "title": "Gita API",
    "description": "AI-native Git server REST API. Standard GitHub-like endpoints plus AI review, semantic search, PR/Issue workflows and an AI-readable event stream.",
    "version": "0.0.1"
  },
  "servers": [
    { "url": "/" }
  ],
  "tags": [
    { "name": "auth", "description": "User registration, login and personal access tokens" },
    { "name": "repos", "description": "Repository CRUD and content browsing" },
    { "name": "commits", "description": "Commit history" },
    { "name": "prs", "description": "Pull request workflow" },
    { "name": "reviews", "description": "AI code reviews" },
    { "name": "issues", "description": "Issue tracking" },
    { "name": "search", "description": "Code search" },
    { "name": "events", "description": "AI-readable event stream" },
    { "name": "misc", "description": "Health and metadata" }
  ],
  "paths": {
    "/api/health": {
      "get": {
        "tags": ["misc"],
        "summary": "Health check",
        "responses": {
          "200": {
            "description": "Server status",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "status": { "type": "string", "enum": ["ok", "degraded"] },
                    "db": { "type": "string", "enum": ["ok", "error"] },
                    "ai_configured": { "type": "boolean" },
                    "git": { "type": "string" }
                  }
                }
              }
            }
          }
        }
      }
    },
    "/api/auth/register": {
      "post": {
        "tags": ["auth"],
        "summary": "Register a new user",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/RegisterRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Registered",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/AuthResponse" }
              }
            }
          },
          "400": { "description": "Bad request" },
          "409": { "description": "Username or email already taken" }
        }
      }
    },
    "/api/auth/login": {
      "post": {
        "tags": ["auth"],
        "summary": "Log in and obtain a JWT",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/LoginRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Authenticated",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/AuthResponse" }
              }
            }
          },
          "401": { "description": "Invalid credentials" }
        }
      }
    },
    "/api/auth/me": {
      "get": {
        "tags": ["auth"],
        "summary": "Get the current authenticated user",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": {
            "description": "Current user",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/PublicUser" }
              }
            }
          },
          "401": { "description": "Not authenticated" }
        }
      }
    },
    "/api/repos": {
      "get": {
        "tags": ["repos"],
        "summary": "List repositories",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "owner", "in": "query", "schema": { "type": "string" }, "description": "Filter by owner username" },
          { "name": "limit", "in": "query", "schema": { "type": "integer" }, "description": "Max results (default 100)" }
        ],
        "responses": {
          "200": {
            "description": "Repository list",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Repository" } }
              }
            }
          }
        }
      },
      "post": {
        "tags": ["repos"],
        "summary": "Create a repository",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/CreateRepoRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Created repository",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Repository" }
              }
            }
          },
          "400": { "description": "Invalid repository name" },
          "409": { "description": "Repository already exists" }
        }
      }
    },
    "/api/repos/{owner}/{repo}": {
      "get": {
        "tags": ["repos"],
        "summary": "Get a repository",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "responses": {
          "200": {
            "description": "Repository",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Repository" }
              }
            }
          },
          "404": { "description": "Not found" }
        }
      },
      "delete": {
        "tags": ["repos"],
        "summary": "Delete a repository (owner only)",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "responses": {
          "200": { "description": "Deleted" },
          "403": { "description": "Forbidden" },
          "404": { "description": "Not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/tree": {
      "get": {
        "tags": ["repos"],
        "summary": "List files in a directory of a repository",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "path", "in": "query", "schema": { "type": "string" }, "description": "Directory path (default root)" },
          { "name": "ref", "in": "query", "schema": { "type": "string" }, "description": "Branch or commit (default default_branch)" }
        ],
        "responses": {
          "200": {
            "description": "Tree entries",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/TreeResponse" }
              }
            }
          },
          "404": { "description": "Repository not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/commits": {
      "get": {
        "tags": ["commits"],
        "summary": "List recent commits",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "ref", "in": "query", "schema": { "type": "string" } },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Commit list",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/CommitsResponse" }
              }
            }
          }
        }
      }
    },
    "/api/repos/{owner}/{repo}/branches": {
      "get": {
        "tags": ["repos"],
        "summary": "List branches",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "responses": {
          "200": {
            "description": "Branch names",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "type": "string" } }
              }
            }
          }
        }
      }
    },
    "/api/repos/{owner}/{repo}/prs": {
      "get": {
        "tags": ["prs"],
        "summary": "List pull requests",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "status", "in": "query", "schema": { "type": "string", "enum": ["open", "merged", "closed", "all"] } }
        ],
        "responses": {
          "200": {
            "description": "PR list",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/PrListResponse" }
              }
            }
          }
        }
      },
      "post": {
        "tags": ["prs"],
        "summary": "Create a pull request",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/CreatePrRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Created PR",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/PullRequest" }
              }
            }
          },
          "400": { "description": "Invalid source/target branch" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}": {
      "get": {
        "tags": ["prs"],
        "summary": "Get a pull request with its AI reviews",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "number", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "PR detail",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/PrDetailResponse" }
              }
            }
          },
          "404": { "description": "PR not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/review": {
      "post": {
        "tags": ["reviews", "prs"],
        "summary": "Trigger an AI review of a pull request",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "number", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "AI review",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Review" }
              }
            }
          }
        }
      }
    },
    "/api/reviews": {
      "get": {
        "tags": ["reviews"],
        "summary": "List AI reviews",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Review list",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Review" } }
              }
            }
          }
        }
      }
    },
    "/api/reviews/{id}": {
      "get": {
        "tags": ["reviews"],
        "summary": "Get an AI review by id",
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Review",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Review" }
              }
            }
          },
          "404": { "description": "Review not found" }
        }
      },
      "patch": {
        "tags": ["reviews"],
        "summary": "Update an AI review (summary / risk_score). Repository owner only.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/UpdateReviewRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Updated review",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Review" }
              }
            }
          },
          "403": { "description": "Forbidden (not the repository owner)" },
          "404": { "description": "Review not found" }
        }
      }
    },
    "/api/reviews/{id}/comments": {
      "get": {
        "tags": ["reviews"],
        "summary": "List user comments on a review",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Comment list",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "review_id": { "type": "integer" },
                    "comments": {
                      "type": "array",
                      "items": { "$ref": "#/components/schemas/ReviewDiscussionComment" }
                    }
                  }
                }
              }
            }
          },
          "404": { "description": "Review not found" }
        }
      },
      "post": {
        "tags": ["reviews"],
        "summary": "Post a user comment on a review",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/CreateDiscussionCommentRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Created comment",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/ReviewDiscussionComment" }
              }
            }
          },
          "404": { "description": "Review not found" }
        }
      }
    },
    "/api/reviews/comments/{comment_id}": {
      "patch": {
        "tags": ["reviews"],
        "summary": "Edit your own review comment",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "comment_id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/UpdateDiscussionCommentRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Updated comment",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/ReviewDiscussionComment" }
              }
            }
          },
          "403": { "description": "Forbidden (not the comment author)" },
          "404": { "description": "Comment not found" }
        }
      },
      "delete": {
        "tags": ["reviews"],
        "summary": "Delete your own review comment (soft delete)",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "comment_id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Deleted"
          },
          "403": { "description": "Forbidden (not the comment author)" },
          "404": { "description": "Comment not found" }
        }
      }
    },
    "/api/reviews/comments/{comment_id}/resolve": {
      "post": {
        "tags": ["reviews"],
        "summary": "Mark an AI review comment as resolved. Repository owner only.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "comment_id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Resolved" },
          "403": { "description": "Forbidden (not the repository owner)" },
          "404": { "description": "Comment not found" }
        }
      }
    },
    "/api/reviews/comments/{comment_id}/unresolve": {
      "post": {
        "tags": ["reviews"],
        "summary": "Clear the resolved flag on an AI review comment. Repository owner only.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "comment_id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Unresolved" },
          "403": { "description": "Forbidden (not the repository owner)" },
          "404": { "description": "Comment not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/issues": {
      "get": {
        "tags": ["issues"],
        "summary": "List issues",
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "status", "in": "query", "schema": { "type": "string", "enum": ["open", "closed", "all"] } }
        ],
        "responses": {
          "200": {
            "description": "Issue list",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Issue" } }
              }
            }
          }
        }
      },
      "post": {
        "tags": ["issues"],
        "summary": "Create an issue",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/CreateIssueRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Created issue",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Issue" }
              }
            }
          }
        }
      }
    },
    "/api/search": {
      "get": {
        "tags": ["search"],
        "summary": "Search repository code",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "q", "in": "query", "required": true, "schema": { "type": "string" }, "description": "Query (keyword or natural language)" },
          { "name": "repo", "in": "query", "schema": { "type": "string" }, "description": "owner/name to restrict search" },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Search hits",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/SearchResponse" }
              }
            }
          }
        }
      }
    },
    "/api/events": {
      "get": {
        "tags": ["events"],
        "summary": "List the AI-readable event stream",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "repo", "in": "query", "schema": { "type": "string" }, "description": "owner/name to restrict" },
          { "name": "type", "in": "query", "schema": { "type": "string" }, "description": "Event type filter" },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Event list",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Event" } }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "securitySchemes": {
      "bearerAuth": {
        "type": "http",
        "scheme": "bearer",
        "bearerFormat": "JWT or PAT"
      }
    },
    "schemas": {
      "RegisterRequest": {
        "type": "object",
        "required": ["username", "email", "password"],
        "properties": {
          "username": { "type": "string" },
          "email": { "type": "string" },
          "password": { "type": "string", "format": "password" }
        }
      },
      "LoginRequest": {
        "type": "object",
        "required": ["username", "password"],
        "properties": {
          "username": { "type": "string" },
          "password": { "type": "string", "format": "password" }
        }
      },
      "AuthResponse": {
        "type": "object",
        "properties": {
          "token": { "type": "string" },
          "user": { "$ref": "#/components/schemas/PublicUser" }
        }
      },
      "PublicUser": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "username": { "type": "string" },
          "email": { "type": "string" }
        }
      },
      "CreateRepoRequest": {
        "type": "object",
        "required": ["name"],
        "properties": {
          "name": { "type": "string" },
          "description": { "type": "string" },
          "is_private": { "type": "boolean" },
          "default_branch": { "type": "string" }
        }
      },
      "Repository": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "owner": { "type": "string" },
          "name": { "type": "string" },
          "full_name": { "type": "string" },
          "description": { "type": "string" },
          "is_private": { "type": "boolean" },
          "default_branch": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" }
        }
      },
      "TreeResponse": {
        "type": "object",
        "properties": {
          "path": { "type": "string" },
          "ref": { "type": "string" },
          "entries": {
            "type": "array",
            "items": { "$ref": "#/components/schemas/TreeEntry" }
          }
        }
      },
      "TreeEntry": {
        "type": "object",
        "properties": {
          "path": { "type": "string" },
          "name": { "type": "string" },
          "kind": { "type": "string", "enum": ["blob", "tree"] },
          "size": { "type": "integer", "nullable": true }
        }
      },
      "CommitsResponse": {
        "type": "object",
        "properties": {
          "ref": { "type": "string" },
          "commits": { "type": "array", "items": { "$ref": "#/components/schemas/Commit" } }
        }
      },
      "Commit": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "repo_id": { "type": "integer" },
          "sha": { "type": "string" },
          "author_name": { "type": "string" },
          "author_email": { "type": "string" },
          "message": { "type": "string" },
          "parent_shas": { "type": "array", "items": { "type": "string" } },
          "authored_at": { "type": "string", "format": "date-time" }
        }
      },
      "CreatePrRequest": {
        "type": "object",
        "required": ["title", "source_branch"],
        "properties": {
          "title": { "type": "string" },
          "body": { "type": "string" },
          "source_branch": { "type": "string" },
          "target_branch": { "type": "string" }
        }
      },
      "PullRequest": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "number": { "type": "integer" },
          "title": { "type": "string" },
          "body": { "type": "string" },
          "source_branch": { "type": "string" },
          "target_branch": { "type": "string" },
          "status": { "type": "string", "enum": ["open", "merged", "closed"] },
          "author": { "type": "string" },
          "base_sha": { "type": "string" },
          "head_sha": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" },
          "updated_at": { "type": "string", "format": "date-time" }
        }
      },
      "PrListResponse": {
        "type": "object",
        "properties": {
          "prs": { "type": "array", "items": { "$ref": "#/components/schemas/PullRequest" } }
        }
      },
      "PrDetailResponse": {
        "type": "object",
        "properties": {
          "pr": { "$ref": "#/components/schemas/PullRequest" },
          "reviews": { "type": "array", "items": { "$ref": "#/components/schemas/Review" } }
        }
      },
      "Review": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "pr_id": { "type": "integer", "nullable": true },
          "repo_id": { "type": "integer" },
          "commit_sha": { "type": "string" },
          "status": { "type": "string" },
          "risk_score": { "type": "integer" },
          "summary": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" },
          "updated_at": { "type": "string", "format": "date-time" },
          "comments": { "type": "array", "items": { "$ref": "#/components/schemas/ReviewComment" } }
        }
      },
      "ReviewComment": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "review_id": { "type": "integer" },
          "file_path": { "type": "string" },
          "line": { "type": "integer", "nullable": true },
          "severity": { "type": "string", "enum": ["info", "warning", "error"] },
          "message": { "type": "string" },
          "suggestion": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" },
          "resolved": { "type": "boolean" },
          "resolved_by": { "type": "integer", "nullable": true },
          "resolved_at": { "type": "string", "format": "date-time", "nullable": true }
        }
      },
      "ReviewDiscussionComment": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "review_id": { "type": "integer" },
          "author_id": { "type": "integer" },
          "author_name": { "type": "string" },
          "body": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" },
          "updated_at": { "type": "string", "format": "date-time" },
          "deleted_at": { "type": "string", "format": "date-time", "nullable": true }
        }
      },
      "CreateDiscussionCommentRequest": {
        "type": "object",
        "required": ["body"],
        "properties": {
          "body": { "type": "string" }
        }
      },
      "UpdateDiscussionCommentRequest": {
        "type": "object",
        "required": ["body"],
        "properties": {
          "body": { "type": "string" }
        }
      },
      "UpdateReviewRequest": {
        "type": "object",
        "properties": {
          "summary": { "type": "string" },
          "risk_score": { "type": "integer", "minimum": 0, "maximum": 100 }
        }
      },
      "CreateIssueRequest": {
        "type": "object",
        "required": ["title"],
        "properties": {
          "title": { "type": "string" },
          "body": { "type": "string" }
        }
      },
      "Issue": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "number": { "type": "integer" },
          "title": { "type": "string" },
          "body": { "type": "string" },
          "status": { "type": "string", "enum": ["open", "closed"] },
          "author": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" }
        }
      },
      "SearchResponse": {
        "type": "object",
        "properties": {
          "query": { "type": "string" },
          "hits": { "type": "array", "items": { "$ref": "#/components/schemas/SearchHit" } }
        }
      },
      "SearchHit": {
        "type": "object",
        "properties": {
          "repo_id": { "type": "integer" },
          "repo_name": { "type": "string" },
          "file_path": { "type": "string" },
          "content": { "type": "string" },
          "score": { "type": "number", "format": "float" }
        }
      },
      "Event": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "event_type": { "type": "string" },
          "repo_id": { "type": "integer", "nullable": true },
          "actor_id": { "type": "integer", "nullable": true },
          "payload": { "type": "object", "additionalProperties": true },
          "created_at": { "type": "string", "format": "date-time" }
        }
      }
    }
  }
}"##;

/// Swagger UI HTML page that loads the OpenAPI document from
/// `/api/docs/openapi.json` and renders it using the Swagger UI CDN.
pub const SWAGGER_UI_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Gita API Docs</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
  <style>
    body { margin: 0; background: #0d1117; }
    #swagger-ui { max-width: 1460px; margin: 0 auto; }
  </style>
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script>
    window.onload = function () {
      window.ui = SwaggerUIBundle({
        url: '/api/docs/openapi.json',
        dom_id: '#swagger-ui',
        deepLinking: true,
        presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
        layout: 'BaseLayout'
      });
    };
  </script>
</body>
</html>"##;

/// Build the router for the OpenAPI documentation endpoints.
pub fn docs_routes() -> axum::Router<AppState> {
    use axum::routing::get;
    use axum::Router;

    Router::new()
        .route("/api/docs/openapi.json", get(openapi_json))
        .route("/api/docs", get(swagger_ui))
}

async fn openapi_json() -> impl axum::response::IntoResponse {
    axum::Json(serde_json::from_str::<serde_json::Value>(OPENAPI_JSON).unwrap())
}

async fn swagger_ui() -> impl axum::response::IntoResponse {
    axum::response::Html(SWAGGER_UI_HTML)
}