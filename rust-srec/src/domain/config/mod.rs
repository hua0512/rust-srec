//! Configuration domain module.

mod context;
mod merged;
mod resolver;

pub use context::ResolvedStreamerContext;
pub use merged::{MergedConfig, MergedConfigBuilder};
pub use resolver::ConfigResolver;
