//! OpenAPI documentation.
//!
//! Serves a hand-written OpenAPI 3.0 JSON document at
//! `/api/docs/openapi.json` and a Swagger UI HTML page at `/api/docs`.
//! The UI loads the standard Swagger UI assets from a CDN.
//!
//! The same document is rendered into `docs/api.md` — a readable endpoint
//! reference — by [`render_api_reference`]. A test compares the rendered
//! text with the committed file, so the two cannot drift apart.

use crate::AppState;

/// The HTTP methods a path entry can have in an OpenAPI document.
pub const HTTP_METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

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
    { "name": "admin", "description": "Administrator user management" },
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
    "/api/auth/initialized": {
      "get": {
        "tags": ["auth"],
        "summary": "Whether the system has been initialized yet",
        "description": "True once any user account exists. Unauthenticated, so the frontend can decide whether to show the init page before any account exists.",
        "responses": {
          "200": {
            "description": "Initialization status",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "initialized": { "type": "boolean" }
                  }
                }
              }
            }
          }
        }
      }
    },
    "/api/auth/init": {
      "post": {
        "tags": ["auth"],
        "summary": "Initialize the system by creating the first admin account",
        "description": "Only accepted when the users table is completely empty. Can only ever run once. Returns a JWT for the new admin.",
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/InitRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "Initialized",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/AuthResponse" }
              }
            }
          },
          "400": { "description": "Bad request, or the system is already initialized" },
          "409": { "description": "Username or email already in use" }
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
      },
      "patch": {
        "tags": ["auth"],
        "summary": "Update the current user's locale, e-mail or password",
        "description": "Every field is optional, so a body-less PATCH is a no-op. Changing the e-mail or the password requires `current_password`.",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": false,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "locale": { "type": "string", "enum": ["zh", "en"] },
                  "email": { "type": "string", "format": "email" },
                  "current_password": { "type": "string" },
                  "new_password": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": {
          "200": {
            "description": "The updated user",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/PublicUser" }
              }
            }
          },
          "400": { "description": "Invalid locale, e-mail or missing current password" },
          "401": { "description": "Not authenticated, or the current password is wrong" },
          "409": { "description": "The e-mail is already registered" }
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
    "/api/repos/{*path}": {
      "description": "All repository sub-resources are served through one catch-all path. `{*path}` is `owner/repo` optionally followed by a sub-resource suffix. `owner` may be a username or a nested group path (`groupa/subgroupa`); `repo` is the last path segment. Sub-resource suffixes: (empty) repository metadata, `/tree`, `/file`, `/commits`, `/commits/{sha}`, `/branches`, `/branches/{branch}`, `/tags`, `/tags/{tag}`, `/star`, `/fork`, `/hooks`, `/hooks/{id}.",
      "get": {
        "tags": ["repos"],
        "summary": "Repository metadata or a read-only sub-resource",
        "description": "Suffix dispatch: (empty) get the repository, /tree list a directory (query: ref, path), /file read a file (query: ref, path), /commits list commits (query: ref, limit), /commits/{sha} get one commit with its diff, /branches list branches, /tags list tags, /star get star status, /hooks list webhooks.",
        "security": [{}, { "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "owner/repo plus optional sub-resource suffix" }
        ],
        "responses": {
          "200": { "description": "Depends on the suffix: repository, tree entries, file content, commit list, commit detail, branch/tag list, star status, or webhook list" },
          "404": { "description": "Repository, ref, file or endpoint not found" }
        }
      },
      "post": {
        "tags": ["repos"],
        "summary": "Create a sub-resource",
        "description": "Suffix dispatch: /fork fork the repository under the caller's namespace (optional body: {name}), /hooks create a webhook, /branches create a branch ({name, ref}), /tags create a tag ({name, ref}).",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "owner/repo plus suffix /fork, /hooks, /branches, or /tags" }
        ],
        "responses": {
          "200": { "description": "Created: fork, webhook, branch or tag" },
          "400": { "description": "Invalid request body or name" },
          "403": { "description": "Not allowed to write to this repository" },
          "404": { "description": "Repository or endpoint not found" },
          "409": { "description": "Destination repository already exists" }
        }
      },
      "put": {
        "tags": ["repos"],
        "summary": "Star a repository",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "owner/repo plus suffix /star" }
        ],
        "responses": {
          "200": { "description": "Starred; returns {starred, new, count}" },
          "401": { "description": "Not authenticated" },
          "404": { "description": "Repository not found" }
        }
      },
      "patch": {
        "tags": ["repos"],
        "summary": "Update a repository (owner only)",
        "description": "Change the description, the visibility, the name, or transfer the repository to another user or group. A non-owner gets 404, not 403: the refusal must not disclose that the repository exists.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "owner/repo" }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": {
                  "description": { "type": "string" },
                  "is_private": { "type": "boolean" },
                  "new_name": { "type": "string", "description": "Rename within the same namespace" },
                  "new_owner": { "type": "string", "description": "Transfer target: a username or a group name" }
                }
              }
            }
          }
        },
        "responses": {
          "200": {
            "description": "The updated repository",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Repository" }
              }
            }
          },
          "400": { "description": "Invalid name" },
          "404": { "description": "Not found, or not the repository's owner" },
          "409": { "description": "The target name is already taken" }
        }
      },
      "delete": {
        "tags": ["repos"],
        "summary": "Delete a repository or a sub-resource (owner only)",
        "description": "Suffix dispatch: (empty) delete the repository, /star unstar, /hooks/{id} delete a webhook, /branches/{branch} delete a branch, /tags/{tag} delete a tag.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "owner/repo plus optional sub-resource suffix" }
        ],
        "responses": {
          "200": { "description": "Deleted" },
          "404": { "description": "Not found, or not the repository's owner" }
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
        "description": "No authentication required. An anonymous caller only sees public repositories; an authenticated one also sees the private repositories they may read.",
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
        "description": "No authentication required. An anonymous caller only sees public repositories.",
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
        "description": "No authentication required. The search only covers the repositories the caller may read, so an anonymous caller only ever matches public code.",
        "parameters": [
          { "name": "q", "in": "query", "required": true, "schema": { "type": "string" } },
          { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ],
        "responses": {
          "200": { "description": "Code hits" }
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
          "404": { "description": "Repository not found, or not readable by the caller" }
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
          "404": { "description": "Repository not found, or not the repository's owner" }
        }
      },
      "delete": {
        "tags": ["repos"],
        "summary": "Disable Pages for a repository",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Pages disabled" },
          "404": { "description": "Repository not found, or not the repository's owner" }
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
                  "description": { "type": "string" },
                  "parent_path": { "type": "string" }
                }
              }
            }
          }
        },
        "responses": { "200": { "description": "Group created" } }
      }
    },
    "/api/groups/{*path}": {
      "get": {
        "tags": ["groups"],
        "summary": "Get a group and its repositories, or list subgroups/members",
        "description": "Use path suffix to select: (empty) for group detail, /subgroups for subgroup list, /members for member list.",
        "security": [{}, { "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Full path of the group, optionally with suffix /subgroups or /members" }
        ],
        "responses": {
          "200": { "description": "Group detail, subgroup list, or member list" },
          "404": { "description": "Group not found" }
        }
      },
      "post": {
        "tags": ["groups"],
        "summary": "Create a repository or subgroup, or add a member",
        "description": "Use path suffix to select action: /repos to create repo, /subgroups to create subgroup, /members to add member.",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Full path of the group with suffix /repos, /subgroups, or /members" }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "oneOf": [
                { "$ref": "#/components/schemas/CreateGroupRepoRequest" },
                { "$ref": "#/components/schemas/CreateGroupRequest" },
                { "$ref": "#/components/schemas/AddGroupMemberRequest" }
              ]
            }
          }
        },
        "responses": {
          "200": { "description": "Repository, subgroup, or member created" },
          "403": { "description": "Not the group owner" },
          "404": { "description": "Group not found" }
        }
      },
      "patch": {
        "tags": ["groups"],
        "summary": "Update a group member's role",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Full path of the group with suffix /members/{username}" }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/UpdateGroupMemberRequest" }
            }
          }
        },
        "responses": {
          "200": { "description": "Member updated" },
          "403": { "description": "Not the group owner" },
          "404": { "description": "Group or member not found" }
        }
      },
      "delete": {
        "tags": ["groups"],
        "summary": "Remove a group member",
        "security": [{ "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "Full path of the group with suffix /members/{username}" }
        ],
        "responses": {
          "200": { "description": "Member removed" },
          "403": { "description": "Not the group owner" },
          "404": { "description": "Group or member not found" }
        }
      }
    },
    "/api/resolve/{*path}": {
      "get": {
        "tags": ["misc"],
        "summary": "Resolve a clean URL path to the entity it addresses",
        "description": "A clean URL (/owner/name) can address a repository or a group full path. Tries the path as a repository first (read access enforced), then as a group. Returns {kind: \"repo\", owner, name, owner_kind} or {kind: \"group\", full_path}; 404 when neither matches.",
        "security": [{}, { "bearerAuth": [] }],
        "parameters": [
          { "name": "path", "in": "path", "required": true, "schema": { "type": "string" }, "description": "The clean URL path to resolve, e.g. gita/tests or gita/tests/myrepo" }
        ],
        "responses": {
          "200": {
            "description": "Resolved entity",
            "content": {
              "application/json": {
                "schema": {
                  "oneOf": [
                    {
                      "type": "object",
                      "properties": {
                        "kind": { "type": "string", "enum": ["repo"] },
                        "owner": { "type": "string" },
                        "name": { "type": "string" },
                        "owner_kind": { "type": "string", "enum": ["user", "group"] }
                      }
                    },
                    {
                      "type": "object",
                      "properties": {
                        "kind": { "type": "string", "enum": ["group"] },
                        "full_path": { "type": "string" }
                      }
                    }
                  ]
                }
              }
            }
          },
          "404": { "description": "No repository or group matches the path" }
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
        "description": "Always returns 200, whether or not the account exists, so the endpoint cannot be used to enumerate accounts. When the server runs with `allow_insecure_password_reset` enabled, the response carries the token in a `reset_token` field; otherwise that field is absent. Rate limited per IP.",
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
        "responses": {
          "200": { "description": "If the account exists, a reset link has been sent" },
          "429": { "description": "Rate limited" }
        }
      }
    },
    "/api/auth/password-reset/confirm": {
      "post": {
        "tags": ["auth"],
        "summary": "Complete a password reset with a token",
        "description": "The token is single-use: it is consumed by the same statement that validates it, so a token can never be redeemed twice. Rate limited per IP.",
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
          "400": { "description": "The new password is too short" },
          "404": { "description": "Invalid, expired or already-used token" },
          "429": { "description": "Rate limited" }
        }
      }
    },
    "/api/auth/logout": {
      "post": {
        "tags": ["auth"],
        "summary": "Log out and revoke the current session",
        "description": "Revokes the JWT used for this request by adding its JTI to the revocation list. The token can no longer be used for authentication. When the request presents a personal access token instead of a JWT, there is no session to end: the response says so and the PAT is left untouched — revoke a PAT explicitly with DELETE /api/auth/tokens/{id}.",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "Logged out" },
          "401": { "description": "Missing or invalid token" }
        }
      }
    },
    "/api/admin/users": {
      "get": {
        "tags": ["admin"],
        "summary": "List all users (admin only)",
        "description": "Requires the caller to be a system administrator. Returns every registered account with its admin flag.",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "User list" },
          "403": { "description": "Caller is not an administrator" }
        }
      },
      "post": {
        "tags": ["admin"],
        "summary": "Create a user (admin only)",
        "description": "Requires the caller to be a system administrator. Creates an account with the given username, email and password; optionally promotes it to administrator.",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/AdminCreateUserRequest" }
            }
          }
        },
        "responses": {
          "200": { "description": "Created user" },
          "400": { "description": "Invalid username, email, password or locale" },
          "403": { "description": "Caller is not an administrator" },
          "409": { "description": "Username or email already in use" }
        }
      }
    },
    "/api/admin/users/{username}": {
      "patch": {
        "tags": ["admin"],
        "summary": "Update a user (admin only)",
        "description": "Requires the caller to be a system administrator. Can change the admin flag, email, locale or password. Refuses to demote the very last administrator.",
        "security": [{ "bearerAuth": [] }],
        "requestBody": {
          "required": false,
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/AdminUpdateUserRequest" }
            }
          }
        },
        "responses": {
          "200": { "description": "Updated user" },
          "400": { "description": "Invalid field value, or attempting to demote the last admin" },
          "403": { "description": "Caller is not an administrator" },
          "404": { "description": "User not found" },
          "409": { "description": "Email already registered" }
        }
      },
      "delete": {
        "tags": ["admin"],
        "summary": "Delete a user (admin only)",
        "description": "Requires the caller to be a system administrator. Deletes the account together with its personal repositories and owned groups. Refuses to delete the last administrator or the caller's own account.",
        "security": [{ "bearerAuth": [] }],
        "responses": {
          "200": { "description": "User deleted" },
          "400": { "description": "Cannot delete the last admin or your own account" },
          "403": { "description": "Caller is not an administrator" },
          "404": { "description": "User not found" }
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
      "Error": {
        "type": "object",
        "description": "The error envelope every failed request returns.",
        "required": ["error"],
        "properties": {
          "error": {
            "type": "object",
            "required": ["message", "status"],
            "properties": {
              "message": { "type": "string", "description": "A human-readable description of what went wrong." },
              "status": { "type": "integer", "description": "The HTTP status code, repeated for convenience." }
            }
          }
        }
      },
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
      "InitRequest": {
        "type": "object",
        "required": ["username", "email", "password"],
        "properties": {
          "username": { "type": "string" },
          "email": { "type": "string" },
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
          "locale": { "type": "string", "enum": ["zh", "en"] },
          "is_admin": { "type": "boolean" }
        }
      },
      "AdminCreateUserRequest": {
        "type": "object",
        "required": ["username", "email", "password"],
        "properties": {
          "username": { "type": "string" },
          "email": { "type": "string" },
          "password": { "type": "string", "format": "password" },
          "locale": { "type": "string", "enum": ["zh", "en"] },
          "is_admin": { "type": "boolean" }
        }
      },
      "AdminUpdateUserRequest": {
        "type": "object",
        "properties": {
          "is_admin": { "type": "boolean" },
          "email": { "type": "string" },
          "locale": { "type": "string", "enum": ["zh", "en"] },
          "password": { "type": "string", "format": "password" }
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

/// Render the OpenAPI document as a Markdown endpoint reference.
///
/// The output is what `docs/api.md` contains. It is generated rather than
/// written by hand so that the reference and the served specification
/// cannot describe different APIs — `cargo run -p gita-api --bin
/// gen-api-docs > docs/api.md` regenerates it, and a test fails when the
/// committed file is out of date.
pub fn render_api_reference() -> String {
    let spec: serde_json::Value =
        serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON should be valid JSON");

    let mut out = String::new();

    out.push_str("# Gita REST API\n\n");
    let description = spec["info"]["description"].as_str().unwrap_or_default();
    out.push_str(description);
    out.push_str("\n\n");

    let version = spec["info"]["version"].as_str().unwrap_or_default();
    out.push_str(&format!(
        "规范版本 `{version}`。运行中的服务器同时提供：\n\n\
         - OpenAPI 文档：`/api/docs/openapi.json`\n\
         - Swagger UI：`/api/docs`\n\n"
    ));

    out.push_str(
        "> 本文件由 `crates/gita-api/src/docs.rs` 里的 OpenAPI 文档生成，\
         不要手工编辑。\n> 重新生成：`cargo run -p gita-api --bin gen-api-docs > docs/api.md`\n\n",
    );

    out.push_str("## 约定\n\n");
    out.push_str(
        "- **认证**：除特别说明外，接口需要 `Authorization: Bearer <token>`。\
         Token 可以是登录得到的 JWT，也可以是个人访问令牌（PAT）。\n\
         - **错误**：所有失败都返回同一个信封 \
         `{\"error\": {\"message\": \"…\", \"status\": 404}}`，`status` 与 HTTP 状态码一致。\n\
         - **拒绝方式**：没有权限访问的仓库一律返回 `404`，而不是 `403`——\
         拒绝本身不应该泄露“这个仓库存在”。\n\
         - **路径参数**：`{owner}` 可以是用户名，也可以是群组名。\n\n",
    );

    // The tag list drives the grouping, so an endpoint without a tag still
    // shows up (under "其它") instead of vanishing from the reference.
    let tags = spec["tags"].as_array().cloned().unwrap_or_default();
    let mut tag_names: Vec<String> = tags
        .iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
        .collect();

    // Collect every operation, keyed by tag.
    let paths = spec["paths"]
        .as_object()
        .expect("paths should be an object");
    let mut by_tag: std::collections::BTreeMap<String, Vec<Operation>> = Default::default();
    for (path, item) in paths {
        let Some(ops) = item.as_object() else {
            continue;
        };
        for (method, op) in ops {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let tag = op["tags"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|t| t.as_str())
                .unwrap_or("misc")
                .to_string();
            if !tag_names.contains(&tag) {
                tag_names.push(tag.clone());
            }
            by_tag.entry(tag).or_default().push(Operation {
                method: method.to_uppercase(),
                path: path.clone(),
                summary: op["summary"].as_str().unwrap_or_default().to_string(),
                description: op["description"].as_str().unwrap_or_default().to_string(),
                auth: op.get("security").is_some(),
                responses: op["responses"]
                    .as_object()
                    .map(|r| r.keys().map(|k| k.as_str().to_string()).collect::<Vec<_>>())
                    .unwrap_or_default(),
                query: op["parameters"]
                    .as_array()
                    .map(|params| {
                        params
                            .iter()
                            .filter(|p| p["in"].as_str() == Some("query"))
                            .filter_map(|p| p["name"].as_str())
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            });
        }
    }

    for tag in tag_names {
        let Some(ops) = by_tag.get(&tag) else {
            continue;
        };
        if ops.is_empty() {
            continue;
        }
        out.push_str(&format!("## {tag}\n\n"));
        if let Some(desc) = tags
            .iter()
            .find(|t| t["name"].as_str() == Some(tag.as_str()))
            .and_then(|t| t["description"].as_str())
        {
            out.push_str(desc);
            out.push_str("\n\n");
        }
        for op in ops {
            out.push_str(&format!("### `{} {}`\n\n", op.method, op.path));
            if !op.summary.is_empty() {
                out.push_str(&format!("{}\n\n", op.summary));
            }
            if !op.description.is_empty() {
                out.push_str(&format!("{}\n\n", op.description));
            }
            let mut facts: Vec<String> = Vec::new();
            facts.push(if op.auth {
                "需要认证".to_string()
            } else {
                "无需认证".to_string()
            });
            if !op.query.is_empty() {
                facts.push(format!("查询参数：`{}`", op.query.join("`, `")));
            }
            facts.push(format!("响应码：`{}`", op.responses.join("`, `")));
            out.push_str(&format!("- {}\n\n", facts.join("\n- ")));
        }
    }

    out
}

/// One documented operation, flattened out of the OpenAPI path item.
struct Operation {
    method: String,
    path: String,
    summary: String,
    description: String,
    auth: bool,
    responses: Vec<String>,
    query: Vec<String>,
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

    /// `docs/api.md` is generated from the OpenAPI document, so the
    /// reference a reader opens and the specification the server serves
    /// cannot describe different APIs.
    ///
    /// When this fails, regenerate the file:
    ///
    /// ```sh
    /// cargo run -p gita-api --bin gen-api-docs > docs/api.md
    /// ```
    #[test]
    fn api_reference_document_is_up_to_date() {
        const COMMITTED: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/api.md"));
        let rendered = render_api_reference();
        assert_eq!(
            COMMITTED, rendered,
            "docs/api.md is out of date; regenerate it with \
             `cargo run -p gita-api --bin gen-api-docs > docs/api.md`"
        );
    }

    /// The OpenAPI document is a hand-maintained JSON string, so a stray
    /// comma or brace breaks the endpoint at runtime rather than at compile
    /// time.
    #[test]
    fn openapi_json_is_valid() {
        serde_json::from_str::<serde_json::Value>(OPENAPI_JSON)
            .expect("OPENAPI_JSON should be valid JSON");
    }

    /// Every route the API registers should appear in the spec, and every
    /// route the spec describes should exist.
    ///
    /// The spec is maintained by hand, so it drifted: 17 of the 35 API
    /// routes were missing, including the whole pipeline family and every
    /// PR action. Rather than trust a checklist, this compares the spec
    /// against the routes the server actually registers.
    ///
    /// The comparison is on **path *and* method**. Comparing paths alone
    /// missed three combinations whose methods the spec did not have —
    /// including `PATCH /api/auth/me` and `PATCH /api/repos/{owner}/{repo}`,
    /// the two that change a user's own account and a repository's
    /// visibility. A path that is documented for `get` is not
    /// documentation of its `patch`.
    ///
    /// It is also bidirectional: a spec entry for a route that does not
    /// exist is just as wrong as a route with no spec entry, and is
    /// what a client would try to call.
    #[test]
    fn openapi_documents_every_registered_api_route() {
        let spec: serde_json::Value =
            serde_json::from_str(OPENAPI_JSON).expect("OPENAPI_JSON should be valid JSON");
        let paths = spec["paths"]
            .as_object()
            .expect("paths should be an object");

        // What the spec documents: every (path, method) pair.
        let mut documented: std::collections::HashSet<(String, String)> = Default::default();
        for (path, item) in paths {
            let Some(methods) = item.as_object() else {
                continue;
            };
            let normalised = normalize_path(path);
            for method in methods.keys() {
                // `parameters` and the like are not operations.
                if !HTTP_METHODS.contains(&method.as_str()) {
                    continue;
                }
                documented.insert((normalised.clone(), method.to_lowercase()));
            }
        }

        let mut missing: Vec<String> = Vec::new();
        let mut undocumented: Vec<String> = Vec::new();
        let registered_routes = registered_api_routes();
        // The comparison is on the *normalised* path, so that `{owner}`
        // and `{group}` compare equal; the raw path is kept for the
        // error message, which is easier to act on.
        let registered: std::collections::HashSet<(String, String)> = registered_routes
            .iter()
            .map(|(path, method)| (normalize_path(path), method.clone()))
            .collect();
        // A catch-all route (`{*path}`) serves every documented sub-path
        // beneath it: the repo dispatcher routes by suffix, so e.g.
        // `/api/repos/{owner}/{repo}/prs` is served by `GET /api/repos/{*path}`.
        // A documented sub-path counts as covered when a catch-all with the
        // same method prefixes it.
        let catch_alls: Vec<(String, String)> = registered_routes
            .iter()
            .filter(|(path, _)| path.contains("{*"))
            .map(|(path, method)| (normalize_path(path), method.clone()))
            .collect();
        for (path, method) in &registered_routes {
            if !documented.contains(&(normalize_path(path), method.clone())) {
                missing.push(format!("{} {}", method.to_uppercase(), path));
            }
        }
        for (path, method) in &documented {
            if registered.contains(&(path.clone(), method.clone())) {
                continue;
            }
            let covered_by_catch_all = catch_alls
                .iter()
                .any(|(prefix, m)| m == method && path.starts_with(prefix.as_str()));
            if !covered_by_catch_all {
                undocumented.push(format!("{} {}", method.to_uppercase(), path));
            }
        }
        assert!(
            missing.is_empty(),
            "these routes are registered but not documented in the OpenAPI spec: {missing:?}"
        );
        assert!(
            undocumented.is_empty(),
            "the OpenAPI spec documents routes that do not exist: {undocumented:?}"
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

    /// Collect the API routes registered across the route modules, as
    /// `(path, method)` pairs.
    ///
    /// The routes are built with `axum::Router`, whose path table is not
    /// publicly enumerable, so the paths are read out of the source of each
    /// route module. That keeps the check honest without changing how the
    /// router is assembled.
    ///
    /// The method comes from the method router that follows the path:
    ///
    /// ```text
    /// .route("/api/repos", post(create_repo).get(list_repos))
    /// .route("/api/auth/me", axum::routing::get(me).patch(update_me))
    /// ```
    ///
    /// so the text between one `.route(` and the next is scanned for
    /// method names. A handler whose own name ends in a method word
    /// (`create_post`) is not a method: the name has to be followed by
    /// `(` and preceded by something that is not a word character.
    fn registered_api_routes() -> Vec<(String, String)> {
        let mut routes = Vec::new();
        for module in [
            include_str!("routes/auth.rs"),
            include_str!("routes/admin.rs"),
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
            include_str!("routes/resolve.rs"),
        ] {
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
                // `end` is an offset *relative to* `start`; the absolute
                // position of the closing quote is `start + end`.
                let path_end = start + end;
                let path = &module[start..path_end];

                // The method router is the rest of this call's argument
                // list. Scanning stops at the parenthesis that closes it,
                // so a method word in the code that follows cannot be
                // mistaken for one.
                //
                // `after` points at the first character *after* the
                // opening parenthesis of `.route(`, so the parenthesis
                // itself is at `after - 1`.
                let scan_end = matching_paren(module, after - 1).unwrap_or(module.len());

                if path.starts_with("/api/") {
                    for method in methods_in(&module[path_end..scan_end]) {
                        routes.push((path.to_string(), method));
                    }
                }
                // Resume after the closing quote. Using the relative
                // offset here would move `idx` *backwards* and spin this
                // loop forever.
                idx = path_end;
            }
        }
        routes
    }

    /// The index just past the parenthesis that closes the one at `open`.
    ///
    /// Returns `None` when the parentheses do not balance.
    fn matching_paren(text: &str, open: usize) -> Option<usize> {
        let bytes = text.as_bytes();
        if bytes.get(open) != Some(&b'(') {
            return None;
        }
        let mut depth = 0usize;
        for (i, b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The HTTP methods named in a method-router expression.
    fn methods_in(expr: &str) -> Vec<String> {
        let bytes = expr.as_bytes();
        let mut methods = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            // A method name is a word at a word boundary followed by `(`.
            if !bytes[i].is_ascii_alphabetic() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &expr[start..i];
            let at_boundary = start == 0 || !is_word_byte(bytes[start - 1]);
            if at_boundary && i < bytes.len() && bytes[i] == b'(' {
                if let Some(method) = HTTP_METHODS.iter().find(|m| **m == word) {
                    methods.push((*method).to_string());
                }
            }
        }
        methods
    }

    /// Whether a byte can be part of an identifier.
    fn is_word_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
}
