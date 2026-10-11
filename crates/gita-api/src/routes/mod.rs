//! Route modules.

pub mod auth;
pub mod issue;
pub mod misc;
pub mod pr;
pub mod repo;
pub mod review;
pub mod search;

// Re-export route functions.
pub use auth::auth_routes;
pub use repo::repo_routes;
pub use pr::pr_routes;
pub use review::review_routes;
pub use issue::issue_routes;
pub use search::search_routes;
pub use misc::misc_routes;
pub use misc::event_routes;