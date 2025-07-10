use pipeline_common::{Pipeline, PipelineError, Processor, StreamerContext};
use std::sync::Arc;

// Demo data type for our example
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum MediaData {
    Video(Vec<u8>),
    Audio(Vec<u8>),
    Metadata(String),
}

// A simple processor implementation
struct LoggingProcessor {
    name: &'static str,
}

impl LoggingProcessor {
    fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl Processor<MediaData> for LoggingProcessor {
    fn process(
        &mut self,
        input: MediaData,
        output: &mut dyn FnMut(MediaData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        println!("{}: Processing {:?}", self.name, input);
        output(input)?;
        Ok(())
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(MediaData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        println!("{}: Finishing", self.name);
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// A transforming processor
struct MetadataEnricher {
    prefix: String,
}

impl MetadataEnricher {
    fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Processor<MediaData> for MetadataEnricher {
    fn process(
        &mut self,
        input: MediaData,
        output: &mut dyn FnMut(MediaData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match input {
            MediaData::Metadata(metadata) => {
                let enriched = format!("{}: {}", self.prefix, metadata);
                output(MediaData::Metadata(enriched))?;
            }
            other => output(other)?,
        }
        Ok(())
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(MediaData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        println!("MetadataEnricher: Finishing");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "MetadataEnricher"
    }
}

fn main() -> Result<(), PipelineError> {
    // Create a shared context
    let context = Arc::new(StreamerContext::default());

    // Build a pipeline with multiple processors
    let pipeline = Pipeline::new(context)
        .add_processor(LoggingProcessor::new("Logger1"))
        .add_processor(MetadataEnricher::new("ENRICHED"))
        .add_processor(LoggingProcessor::new("Logger2"));

    // Create some example data
    let data: Vec<Result<MediaData, PipelineError>> = vec![
        Ok(MediaData::Video(vec![1, 2, 3])),
        Ok(MediaData::Audio(vec![4, 5, 6])),
        Ok(MediaData::Metadata("Stream info".to_string())),
        Ok(MediaData::Video(vec![7, 8, 9])),
    ];

    // Process the data through the pipeline
    let mut results = Vec::new();
    pipeline.process(data.into_iter(), &mut |result| {
        results.push(result);
    })?;

    // Print the results
    println!("\nResults from pipeline:");
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(data) => println!("Item {i}: {data:?}"),
            Err(err) => println!("Error on item {i}: {err:?}"),
        }
    }

    Ok(())
}
