use std::sync::Arc;

use super::MergedConfig;

/// Resolved streamer context: the merged configuration, including the
/// credential policy, for one streamer.
pub struct ResolvedStreamerContext {
    pub config: Arc<MergedConfig>,
}
