//! Shared CI library for admin CLI and orchestrator.

pub mod image;
pub mod services;
pub mod template;
pub mod userdata;

pub use image::{ImageClient, RegisterImageRequest, ImageInfo};
pub use template::{TemplateClient, CreateTemplateRequest, TemplateInfo};
pub use userdata::generate_vm_job_userdata;