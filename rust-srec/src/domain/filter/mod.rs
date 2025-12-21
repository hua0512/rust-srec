//! Filter domain module.

mod evaluator;
mod logic;
mod types;

pub use evaluator::{EvalContext, FilterEvalError, FilterEvaluator};
pub use logic::{FilterContext, FilterSet};
pub use types::{
    CategoryFilter, CronFilter, Filter, FilterType, KeywordFilter, RegexFilter, TimeBasedFilter,
};
