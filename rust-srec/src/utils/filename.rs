//! Filename sanitization and path template expansion utilities.
//!
//! Canonical definitions live in [`crate::domain::filename`].

pub use crate::domain::filename::{
    expand_placeholders, expand_placeholders_at, sanitize_filename, sanitize_filename_for_template,
    sanitize_title_for_template,
};
