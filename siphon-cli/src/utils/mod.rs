mod headers;
pub mod progress;
mod size;
mod time;

// Export utility functions
pub use self::headers::parse_headers;
pub use self::size::format_bytes;
pub use self::size::parse_size;
pub use self::time::format_duration;
pub use self::time::parse_time;
