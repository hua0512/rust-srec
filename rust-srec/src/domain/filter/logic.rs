//! Filter evaluation logic.

use super::types::Filter;
use chrono::{DateTime, Utc};

/// Context for filter evaluation.
#[derive(Debug, Clone)]
pub struct FilterContext {
    /// Current time for time-based filters.
    pub current_time: DateTime<Utc>,
    /// Stream title for keyword filters.
    pub title: Option<String>,
    /// Stream category for category filters.
    pub category: Option<String>,
}

impl FilterContext {
    /// Create a new filter context with the current time.
    pub fn new() -> Self {
        Self {
            current_time: Utc::now(),
            title: None,
            category: None,
        }
    }

    /// Set the title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set the time.
    pub fn with_time(mut self, time: DateTime<Utc>) -> Self {
        self.current_time = time;
        self
    }
}

impl Default for FilterContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A set of filters that are combined with AND logic.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    filters: Vec<Filter>,
}

impl FilterSet {
    /// Create an empty filter set.
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
        }
    }

    /// Create a filter set from a vector of filters.
    pub fn from_filters(filters: Vec<Filter>) -> Self {
        Self { filters }
    }

    /// Add a filter to the set.
    pub fn add(&mut self, filter: Filter) {
        self.filters.push(filter);
    }

    /// Check if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Get the number of filters.
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Evaluate all filters against the context.
    /// Returns true if ALL filters pass (AND logic).
    /// Returns true if there are no filters.
    pub fn should_record(&self, context: &FilterContext) -> bool {
        if self.filters.is_empty() {
            return true;
        }

        for filter in &self.filters {
            if !self.evaluate_filter(filter, context) {
                return false;
            }
        }

        true
    }

    /// Evaluate a single filter.
    fn evaluate_filter(&self, filter: &Filter, context: &FilterContext) -> bool {
        match filter {
            Filter::TimeBased(tf) => tf.matches(context.current_time),
            Filter::Keyword(kf) => {
                match &context.title {
                    Some(title) => kf.matches(title),
                    None => true, // No title = pass keyword filter
                }
            }
            Filter::Category(cf) => {
                match &context.category {
                    Some(category) => cf.matches(category),
                    None => true, // No category = pass category filter
                }
            }
            Filter::Cron(cf) => cf.matches(context.current_time),
            Filter::Regex(rf) => {
                match &context.title {
                    Some(title) => rf.matches(title),
                    None => true, // No title = pass regex filter
                }
            }
        }
    }

    /// Get a reference to the filters.
    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::filter::{CategoryFilter, KeywordFilter};

    #[test]
    fn test_empty_filter_set() {
        let set = FilterSet::new();
        let context = FilterContext::new();
        assert!(set.should_record(&context));
    }

    #[test]
    fn test_single_keyword_filter() {
        let mut set = FilterSet::new();
        set.add(Filter::Keyword(KeywordFilter::new(
            vec!["live".to_string()],
            vec![],
        )));

        let context = FilterContext::new().with_title("Going live!");
        assert!(set.should_record(&context));

        let context = FilterContext::new().with_title("Rerun");
        assert!(!set.should_record(&context));
    }

    #[test]
    fn test_multiple_filters_and_logic() {
        let mut set = FilterSet::new();
        set.add(Filter::Keyword(KeywordFilter::new(
            vec!["live".to_string()],
            vec![],
        )));
        set.add(Filter::Category(CategoryFilter::new(vec![
            "Just Chatting".to_string(),
        ])));

        // Both pass
        let context = FilterContext::new()
            .with_title("Going live!")
            .with_category("Just Chatting");
        assert!(set.should_record(&context));

        // Keyword passes, category fails
        let context = FilterContext::new()
            .with_title("Going live!")
            .with_category("Gaming");
        assert!(!set.should_record(&context));

        // Keyword fails
        let context = FilterContext::new()
            .with_title("Rerun")
            .with_category("Just Chatting");
        assert!(!set.should_record(&context));
    }

    #[test]
    fn test_filter_with_missing_context() {
        let mut set = FilterSet::new();
        set.add(Filter::Keyword(KeywordFilter::new(
            vec!["live".to_string()],
            vec![],
        )));

        // No title in context - should pass
        let context = FilterContext::new();
        assert!(set.should_record(&context));
    }
}
