//! Configuration domain module.

mod merged;
mod resolver;

pub use merged::{MergedConfig, MergedConfigBuilder};
pub use resolver::ConfigResolver;
