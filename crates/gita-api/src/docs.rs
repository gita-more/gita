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