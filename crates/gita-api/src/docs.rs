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
    "description": "AI-native Git server REST API. Standard GitHub-like endpoints, PR/Issue workflows and an AI-readable event stream.",
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
    { "name": "issues", "description": "Issue tracking" },
    { "name": "events", "description": "AI-readable event stream" },
    { "name": "groups", "description": "Groups and membership" },
    { "name": "search", "description": "Repository and code search" },
    { "name": "pipelines", "description": "In-repo CI pipelines, runs and job logs" },
    { "name": "pages", "description": "Static site (Pages) publishing configuration" },
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
    "/api/auth/keys": {
      "get": {
        "tags": ["auth"],
        "summary": "List SSH public keys of the current user",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": {
            "description": "SSH key list",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "keys": {
                      "type": "array",
                      "items": { "$ref": "#/components/schemas/SshKey" }
                    }
                  }
                }
              }
            }
          },
          "401": { "description": "Not authenticated" }
        }
      },
      "post": {
        "tags": ["auth"],
        "summary": "Register a new SSH public key",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/AddSshKeyRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Registered key",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/SshKey" }
              }
            }
          },
          "400": { "description": "Unsupported key algorithm or missing blob" },
          "401": { "description": "Not authenticated" }
        }
      }
    },
    "/api/auth/keys/{id}": {
      "delete": {
        "tags": ["auth"],
        "summary": "Delete an SSH public key",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": {
            "description": "Deleted",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "message": { "type": "string" }
                  }
                }
              }
            }
          },
          "401": { "description": "Not authenticated" },
          "404": { "description": "Key not found" }
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
        "summary": "Get a pull request",
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
    },
    "/api/search": {
      "get": {
        "tags": ["search"],
        "summary": "Search repositories and code",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "q", "in": "query", "required": true, "schema": { "type": "string" } },
          { "name": "type", "in": "query", "schema": { "type": "string", "enum": ["repos", "code", "all"] } },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Search results" },
          "400": { "description": "Missing query" }
        }
      }
    },
    "/api/search/repos": {
      "get": {
        "tags": ["search"],
        "summary": "Search repositories by name/description",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "q", "in": "query", "required": true, "schema": { "type": "string" } },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Repository hits" }
        }
      }
    },
    "/api/search/code": {
      "get": {
        "tags": ["search"],
        "summary": "Search code in all repositories",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "q", "in": "query", "required": true, "schema": { "type": "string" } },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Code hits" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/commits/{sha}": {
      "get": {
        "tags": ["repos"],
        "summary": "Get a single commit with its diff",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "owner", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "repo", "in": "path", "required": true, "schema": { "type": "string" } },
          { "name": "sha", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "responses": {
          "200": { "description": "Commit detail + diff" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/star": {
      "get": {
        "tags": ["repos"],
        "summary": "Check if the authenticated user starred this repo",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Star status" } }
      },
      "put": {
        "tags": ["repos"],
        "summary": "Star a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Starred" } }
      },
      "delete": {
        "tags": ["repos"],
        "summary": "Unstar a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Unstarred" } }
      }
    },
    "/api/repos/{owner}/{repo}/fork": {
      "post": {
        "tags": ["repos"],
        "summary": "Fork this repository under the caller's namespace",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "name": { "type": "string", "description": "Optional fork name" }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Fork created" },
          "409": { "description": "Destination already exists" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/hooks": {
      "get": {
        "tags": ["repos"],
        "summary": "List webhooks for a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Hook list" } }
      },
      "post": {
        "tags": ["repos"],
        "summary": "Create a webhook",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "url": { "type": "string" },
                  "events": { "type": "array", "items": { "type": "string" } },
                  "secret": { "type": "string" },
                  "active": { "type": "boolean" }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Hook created" },
          "400": { "description": "Invalid URL or events" },
          "403": { "description": "Not the owner" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/hooks/{id}": {
      "delete": {
        "tags": ["repos"],
        "summary": "Delete a webhook",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Hook deleted" },
          "404": { "description": "Hook not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/pages": {
      "get": {
        "tags": ["repos"],
        "summary": "Get the Pages publishing configuration for a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Pages configuration (defaults when unset)" },
          "403": { "description": "Private repository, not the owner" }
        }
      },
      "put": {
        "tags": ["repos"],
        "summary": "Set the Pages publishing configuration",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "enabled": { "type": "boolean" },
                  "branch": { "type": "string" },
                  "path": { "type": "string", "enum": ["/", "/docs"] }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Pages configuration saved" },
          "400": { "description": "Invalid branch or path" },
          "403": { "description": "Not the owner" },
          "404": { "description": "Repository not found" }
        }
      },
      "delete": {
        "tags": ["repos"],
        "summary": "Disable Pages for a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Pages disabled" },
          "403": { "description": "Not the owner" }
        }
      }
    },
    "/api/groups/{group}/members": {
      "get": {
        "tags": ["groups"],
        "summary": "List group members",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Member list" } }
      },
      "post": {
        "tags": ["groups"],
        "summary": "Add a member",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "username": { "type": "string" },
                  "role": { "type": "string", "enum": ["owner", "maintainer", "developer", "member", "guest"] }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Member added" },
          "403": { "description": "Not the group owner" }
        }
      }
    },
    "/api/groups/{group}/members/{username}": {
      "patch": {
        "tags": ["groups"],
        "summary": "Update a member's role",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "role": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": { "200": { "description": "Role updated" } }
      },
      "delete": {
        "tags": ["groups"],
        "summary": "Remove a member",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Member removed" },
          "404": { "description": "Not a member" }
        }
      }
    },
    "/api/auth/tokens": {
      "get": {
        "tags": ["auth"],
        "summary": "List personal access tokens",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Token list" } }
      },
      "post": {
        "tags": ["auth"],
        "summary": "Create a personal access token",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": { "name": { "type": "string" } }
              }
            }
          }
        },
        "responses": { "200": { "description": "Token created" } }
      }
    },
    "/api/auth/tokens/{id}": {
      "delete": {
        "tags": ["auth"],
        "summary": "Revoke a personal access token",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Token revoked" },
          "404": { "description": "Token not found" }
        }
      }
    },
    "/api/groups": {
      "get": {
        "tags": ["groups"],
        "summary": "List groups",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Group list" } }
      },
      "post": {
        "tags": ["groups"],
        "summary": "Create a group",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "name": { "type": "string" },
                  "description": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": { "200": { "description": "Group created" } }
      }
    },
    "/api/groups/{group}": {
      "get": {
        "tags": ["groups"],
        "summary": "Get a group and its repositories",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Group detail" },
          "404": { "description": "Group not found" }
        }
      }
    },
    "/api/groups/{group}/repos": {
      "post": {
        "tags": ["groups"],
        "summary": "Create a repository owned by a group",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "name": { "type": "string" },
                  "description": { "type": "string" },
                  "is_private": { "type": "boolean" },
                  "default_branch": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Repository created" },
          "403": { "description": "Not the group owner" }
        }
      }
    },
    "/api/issues": {
      "get": {
        "tags": ["issues"],
        "summary": "List issues across repositories",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Issue list" } }
      }
    },
    "/api/mcp/tools": {
      "get": {
        "tags": ["misc"],
        "summary": "List the tools exposed to MCP clients",
        "responses": { "200": { "description": "Tool list" } }
      }
    },
    "/api/repos/{owner}/{repo}/file": {
      "get": {
        "tags": ["repos"],
        "summary": "Read a file's content at a ref",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "File content" },
          "404": { "description": "File not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/tags": {
      "get": {
        "tags": ["repos"],
        "summary": "List tags",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Tag list" } }
      },
      "post": {
        "tags": ["repos"],
        "summary": "Create a tag",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "name": { "type": "string" },
                  "ref": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": { "200": { "description": "Tag created" } }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/merge": {
      "post": {
        "tags": ["prs"],
        "summary": "Merge a pull request",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Merged" },
          "409": { "description": "Merge conflict" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/close": {
      "post": {
        "tags": ["prs"],
        "summary": "Close a pull request",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Closed" } }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/diff": {
      "get": {
        "tags": ["prs"],
        "summary": "Get a pull request's diff",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Unified diff" } }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/files": {
      "get": {
        "tags": ["prs"],
        "summary": "List the files a pull request changes",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Changed file list" } }
      }
    },
    "/api/repos/{owner}/{repo}/issues/{number}": {
      "get": {
        "tags": ["issues"],
        "summary": "Get an issue",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Issue detail" },
          "404": { "description": "Issue not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/issues/{number}/close": {
      "post": {
        "tags": ["issues"],
        "summary": "Close an issue",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Closed" } }
      }
    },
    "/api/repos/{owner}/{repo}/pages/status": {
      "get": {
        "tags": ["pages"],
        "summary": "Whether the repository has a Pages configuration",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Pages status" } }
      }
    },
    "/api/repos/{owner}/{repo}/pipelines": {
      "get": {
        "tags": ["pipelines"],
        "summary": "List the pipelines defined in the repository's .gita-ci.yml",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Pipeline list" } }
      }
    },
    "/api/repos/{owner}/{repo}/pipelines/{path}/runs": {
      "get": {
        "tags": ["pipelines"],
        "summary": "List runs of a pipeline",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Run list" } }
      },
      "post": {
        "tags": ["pipelines"],
        "summary": "Trigger a run of a pipeline",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Run enqueued" },
          "404": { "description": "No such pipeline" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/runs/{run_id}": {
      "get": {
        "tags": ["pipelines"],
        "summary": "Get a run with its jobs",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Run detail" },
          "404": { "description": "Run not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/runs/{run_id}/cancel": {
      "post": {
        "tags": ["pipelines"],
        "summary": "Cancel a run",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Cancelled" },
          "404": { "description": "Run not found" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs": {
      "get": {
        "tags": ["pipelines"],
        "summary": "Get a job's captured logs",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Log lines" } }
      }
    },
    "/api/repos/{owner}/{repo}/runs/{run_id}/jobs/{job_id}/logs/stream": {
      "get": {
        "tags": ["pipelines"],
        "summary": "Stream a job's logs as they are produced (SSE)",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "text/event-stream" } }
      }
    },
    "/api/users/me/avatar": {
      "post": {
        "tags": ["misc"],
        "summary": "Upload the current user's avatar image",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "multipart/form-data": {
              "schema": {
                "type": "object",
                "properties": { "file": { "type": "string", "format": "binary" } }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Avatar uploaded" },
          "400": { "description": "Not an image, or too large" }
        }
      }
    },
    "/api/users/{username}/avatar": {
      "get": {
        "tags": ["misc"],
        "summary": "Fetch a user's avatar image",
        "responses": {
          "200": { "description": "Avatar image" },
          "404": { "description": "No avatar" }
        }
      }
    },
    "/api/auth/password-reset/request": {
      "post": {
        "tags": ["auth"],
        "summary": "Request a password-reset link",
        "description": "Always returns 200, whether or not the account exists, so the endpoint cannot be used to enumerate accounts.",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": { "identifier": { "type": "string" } }
              }
            }
          }
        },
        "responses": { "200": { "description": "If the account exists, a reset link has been sent" } }
      }
    },
    "/api/auth/password-reset/confirm": {
      "post": {
        "tags": ["auth"],
        "summary": "Complete a password reset with a token",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "token": { "type": "string" },
                  "new_password": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": {
          "200": { "description": "Password reset" },
          "400": { "description": "Invalid or expired token" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/branches/{branch}": {
      "delete": {
        "tags": ["repos"],
        "summary": "Delete a branch",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Branch deleted" },
          "403": { "description": "Not the owner" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/tags/{tag}": {
      "delete": {
        "tags": ["repos"],
        "summary": "Delete a tag",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Tag deleted" },
          "403": { "description": "Not the owner" }
        }
      }
    },
    "/api/repos/{owner}/{repo}/prs/{number}/comments": {
      "get": {
        "tags": ["prs"],
        "summary": "List a pull request's comments",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Comment list" } }
      },
      "post": {
        "tags": ["prs"],
        "summary": "Comment on a pull request",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Comment created" } }
      }
    },
    "/api/repos/{owner}/{repo}/issues/{number}/comments": {
      "get": {
        "tags": ["issues"],
        "summary": "List an issue's comments",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Comment list" } }
      },
      "post": {
        "tags": ["issues"],
        "summary": "Comment on an issue",
        "security": [{ "bearerAuth": [] }],
        "responses": { "200": { "description": "Comment created" } }
      }
    },
    "/api/comments/{id}": {
      "delete": {
        "tags": ["misc"],
        "summary": "Delete a comment",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Comment deleted" },
          "404": { "description": "Comment not found" }
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
          "email": { "type": "string" },
          "locale": { "type": "string", "enum": ["zh", "en"] }
        }
      },
      "SshKey": {
        "type": "object",
        "required": ["id", "title", "public_key", "created_at"],
        "properties": {
          "id": { "type": "integer" },
          "title": { "type": "string" },
          "public_key": { "type": "string", "description": "OpenSSH authorized_keys line" },
          "created_at": { "type": "string", "format": "date-time" }
        }
      },
      "AddSshKeyRequest": {
        "type": "object",
        "required": ["public_key"],
        "properties": {
          "title": { "type": "string" },
          "public_key": { "type": "string", "description": "OpenSSH public key (e.g. ssh-ed25519 AAAA...)" }
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
          "pr": { "$ref": "#/components/schemas/PullRequest" }
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
      },
      "Pipeline": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "repo_id": { "type": "integer" },
          "path": { "type": "string" },
          "ref": { "type": "string" },
          "ref_sha": { "type": "string" },
          "content": { "type": "string" },
          "created_at": { "type": "string", "format": "date-time" },
          "updated_at": { "type": "string", "format": "date-time" }
        }
      },
      "PipelineRun": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "pipeline_id": { "type": "integer" },
          "repo_id": { "type": "integer" },
          "ref": { "type": "string" },
          "head_sha": { "type": "string" },
          "trigger_event_id": { "type": "integer", "nullable": true },
          "trigger_pusher_id": { "type": "integer", "nullable": true },
          "status": { "type": "string", "enum": ["queued","running","success","failed","canceled"] },
          "queued_at": { "type": "string", "format": "date-time" },
          "started_at": { "type": "string", "format": "date-time", "nullable": true },
          "finished_at": { "type": "string", "format": "date-time", "nullable": true }
        }
      },
      "Job": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "run_id": { "type": "integer" },
          "name": { "type": "string" },
          "stage": { "type": "string" },
          "needs": { "type": "array", "items": { "type": "string" } },
          "script": { "type": "array", "items": { "type": "string" } },
          "status": { "type": "string", "enum": ["queued","running","success","failed","skipped","canceled"] },
          "queued_at": { "type": "string", "format": "date-time" },
          "started_at": { "type": "string", "format": "date-time", "nullable": true },
          "finished_at": { "type": "string", "format": "date-time", "nullable": true },
          "exit_code": { "type": "integer", "nullable": true }
        }
      },
      "JobLogLine": {
        "type": "object",
        "properties": {
          "seq": { "type": "integer" },
          "stream": { "type": "string", "enum": ["stdout","stderr","system"] },
          "line": { "type": "string" },
          "ts": { "type": "string", "format": "date-time" }
        }
      },
      "Comment": {
        "type": "object",
        "properties": {
          "id": { "type": "integer" },
          "repo_id": { "type": "integer" },
          "pr_id": { "type": "integer", "nullable": true },
          "issue_id": { "type": "integer", "nullable": true },
          "author": { "type": "string" },
          "body": { "type": "string" },
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
#[cfg(test)]
mod tests {
    use super::*;

    /// The OpenAPI document is a hand-maintained JSON string, so a stray
    /// comma or brace breaks the endpoint at runtime rather than at compile
    /// time.
    #[test]
    fn openapi_json_is_valid() {
        serde_json::from_str::<serde_json::Value>(OPENAPI_JSON)
            .expect("OPENAPI_JSON should be valid JSON");
    }

    /// Every route the API registers should appear in the spec.
    ///
    /// The spec is maintained by hand, so it drifted: 17 of the 35 API
    /// routes were missing, including the whole pipeline family and every
    /// PR action. Rather than trust a checklist, this compares the spec
    /// against the routes the server actually registers.
    #[test]
    fn openapi_documents_every_registered_api_route() {
        let spec: serde_json::Value =
            serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON should be valid JSON");
        let paths = spec["paths"].as_object().expect("paths should be an object");

        // Every path key in the spec, with parameter names normalised away
        // so `{owner}` and `{group}` compare equal.
        let documented: std::collections::HashSet<String> = paths
            .keys()
            .map(|p| normalize_path(p))
            .collect();

        let mut missing: Vec<String> = Vec::new();
        for route in registered_api_routes() {
            if !documented.contains(&normalize_path(&route)) {
                missing.push(route);
            }
        }
        assert!(
            missing.is_empty(),
            "these routes are registered but not documented in the OpenAPI spec: {missing:?}"
        );
    }

    /// Normalise `{param}` placeholders so paths compare by shape.
    fn normalize_path(path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        let mut in_param = false;
        for ch in path.chars() {
            match ch {
                '{' => in_param = true,
                '}' => {
                    in_param = false;
                    out.push_str("{}");
                }
                _ if !in_param => out.push(ch),
                _ => {}
            }
        }
        out
    }

    /// Collect the API route paths registered across the route modules.
    ///
    /// The routes are built with `axum::Router`, whose path table is not
    /// publicly enumerable, so the paths are read out of the source of each
    /// route module. That keeps the check honest without changing how the
    /// router is assembled.
    fn registered_api_routes() -> Vec<String> {
        let mut routes = Vec::new();
        for module in [
            include_str!("routes/auth.rs"),
            include_str!("routes/repo.rs"),
            include_str!("routes/pr.rs"),
            include_str!("routes/issue.rs"),
            include_str!("routes/comment.rs"),
            include_str!("routes/group.rs"),
            include_str!("routes/pipeline.rs"),
            include_str!("routes/search.rs"),
            include_str!("routes/avatar.rs"),
            include_str!("routes/ssh_key.rs"),
            include_str!("routes/pages.rs"),
            include_str!("routes/misc.rs"),
        ] {
            // A route path is the first string literal following a
            // `.route(` call. The literal is often on the next line, so
            // scan forward from each `.route(` rather than line by line.
            let bytes = module.as_bytes();
            let mut idx = 0;
            while let Some(pos) = module[idx..].find(".route(") {
                let after = idx + pos + ".route(".len();
                let Some(quote) = module[after..].find('"') else {
                    break;
                };
                let start = after + quote + 1;
                let Some(end) = module[start..].find('"') else {
                    break;
                };
                let path = &module[start..start + end];
                if path.starts_with("/api/") {
                    routes.push(path.to_string());
                }
                idx = start + end;
                let _ = bytes;
            }
        }
        routes
    }
}
