//! Route modules.

pub mod auth;
pub mod avatar;
pub mod comment;
pub mod group;
pub mod issue;
pub mod misc;
pub mod pages;
pub mod pipeline;
pub mod pr;
pub mod repo;
pub mod search;
pub mod ssh_key;

// Re-export route functions.
pub use auth::auth_routes;
#[allow(unused_imports)]
pub use auth::auth_routes_except_login;
pub use avatar::avatar_routes;
pub use comment::comment_routes;
pub use group::group_routes;
pub use repo::repo_routes;
pub use pr::pr_routes;
pub use issue::issue_routes;
pub use misc::misc_routes;
pub use misc::event_routes;
pub use pages::pages_routes;
pub use pipeline::pipeline_routes;
pub use search::search_routes;
pub use ssh_key::ssh_key_routes;