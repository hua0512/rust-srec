//! Filter domain module.

mod logic;
mod types;

pub use logic::{FilterContext, FilterSet};
pub use types::{CategoryFilter, Filter, FilterType, KeywordFilter, TimeBasedFilter};
