use flv::error::FlvError;
use pipeline_common::PipelineError;

// Wrapper type for FlvError
pub struct FlvErrorWrapper(pub FlvError);

// Implement From for the wrapper
impl From<FlvErrorWrapper> for PipelineError {
    fn from(wrapper: FlvErrorWrapper) -> Self {
        match wrapper.0 {
            FlvError::InvalidHeader => PipelineError::InvalidData("Invalid FLV header".into()),
            FlvError::Io(io) => PipelineError::Io(io),
            FlvError::IncompleteData => {
                PipelineError::InvalidData("Incomplete data provided to decoder".into())
            }
            FlvError::TagParseError(msg) => {
                PipelineError::InvalidData(format!("Error parsing tag data: {msg}"))
            }
            FlvError::ResyncFailed => {
                PipelineError::InvalidData("Resynchronization failed to find valid tag".into())
            }
            FlvError::InvalidTagType(tag_type) => {
                PipelineError::InvalidData(format!("Invalid tag type encountered: {tag_type}"))
            }
            FlvError::TagTooLarge(size) => {
                PipelineError::InvalidData(format!("Tag data size too large: {size}"))
            }
        }
    }
}

// Convert FlvError to PipelineError
pub fn flv_error_to_pipeline_error(error: FlvError) -> PipelineError {
    FlvErrorWrapper(error).into()
}
