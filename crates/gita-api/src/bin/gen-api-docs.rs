//! Regenerate `docs/api.md` from the OpenAPI document.
//!
//! Run it from the repository root:
//!
//! ```sh
//! cargo run -p gita-api --bin gen-api-docs > docs/api.md
//! ```
//!
//! The output is checked in, and a test in `gita-api` fails when the
//! committed file no longer matches — so the reference and the served
//! specification cannot drift apart.

fn main() {
    print!("{}", gita_api::docs::render_api_reference());
}
