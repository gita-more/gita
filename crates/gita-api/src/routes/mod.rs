//! Route modules.

pub mod auth;
pub mod comment;
pub mod group;
pub mod issue;
pub mod misc;
pub mod pipeline;
pub mod pr;
pub mod repo;

// Re-export route functions.
pub use auth::auth_routes;
pub use auth::auth_routes_except_login;
pub use comment::comment_routes;
pub use group::group_routes;
pub use repo::repo_routes;
pub use pr::pr_routes;
pub use issue::issue_routes;
pub use misc::misc_routes;
pub use misc::event_routes;
pub use pipeline::pipeline_routes;