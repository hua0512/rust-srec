//! Pipeline operators for FLV stream processing

pub mod defragment;
pub mod gop_sort;
pub mod header_check;
pub mod limit;
pub mod script_filler;
pub mod script_filter;
pub mod split;
pub mod time_consistency;
pub mod timing_repair;

use crate::context::StreamerContext;
use flv::data::FlvData;
use flv::error::FlvError;
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;

/// Common trait for all FLV stream operators
pub trait FlvOperator {
    /// Get the operator's context
    fn context(&self) -> &Arc<StreamerContext>;

    /// Process the FLV stream asynchronously
    fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) -> impl std::future::Future<Output = ()> + Send;

    /// Get the name of this operator for logging and debugging
    fn name(&self) -> &'static str;

    /// Process a single FLV data item (optional implementation for stateless operators)
    fn process_item(&mut self, item: Result<FlvData, FlvError>) -> Result<FlvData, FlvError> {
        // Default implementation just returns the item unchanged
        item
    }
}

// Re-export common operators
pub use defragment::DefragmentOperator;
pub use gop_sort::GopSortOperator;
pub use header_check::HeaderCheckOperator;
pub use limit::LimitOperator;
pub use script_filler::ScriptKeyframesFillerOperator;
pub use script_filter::ScriptFilterOperator;
pub use split::SplitOperator;
pub use time_consistency::{ContinuityMode, TimeConsistencyOperator};
pub use timing_repair::{RepairStrategy, TimingRepairConfig, TimingRepairOperator};
