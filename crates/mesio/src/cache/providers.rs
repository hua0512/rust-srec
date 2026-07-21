//! # Cache Providers
//!
//! This module contains different cache provider implementations.

// Re-export providers for easier access
pub use self::file::FileCache;
pub use self::memory::MemoryCache;
pub use self::provider::CacheProvider;

// Provider interface
pub mod provider;

// Individual provider implementations
pub mod file;
pub mod memory;
