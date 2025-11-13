use criterion::{Criterion, criterion_group, criterion_main};
use pipeline_common::{CancellationToken, Pipeline, PipelineError, Processor, StreamerContext};
use std::sync::Arc;

// A simple processor that increments a counter.
#[derive(Default)]
struct CounterProcessor;

impl Processor<u32> for CounterProcessor {
    fn process(
        &mut self,
        _context: &Arc<StreamerContext>,
        data: u32,
        output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        output(data + 1)
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(u32) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "CounterProcessor"
    }
}

// A new recursive pipeline implementation.
struct RecursivePipeline<T> {
    processors: Vec<Box<dyn Processor<T>>>,
}

impl<T: Clone> RecursivePipeline<T> {
    fn new() -> Self {
        Self {
            processors: Vec::new(),
        }
    }

    fn add_processor<P: Processor<T> + 'static>(mut self, processor: P) -> Self {
        self.processors.push(Box::new(processor));
        self
    }

    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        data: T,
    ) -> Result<Vec<T>, PipelineError> {
        fn process_recursive<T: Clone>(
            processors: &mut [Box<dyn Processor<T>>],
            context: &Arc<StreamerContext>,
            data: T,
        ) -> Result<Vec<T>, PipelineError> {
            if let Some((processor, rest)) = processors.split_first_mut() {
                let mut results = Vec::new();
                let mut output_fn = |d: T| {
                    results.extend(process_recursive(rest, context, d.clone())?);
                    Ok(())
                };
                processor.process(context, data, &mut output_fn)?;
                Ok(results)
            } else {
                Ok(vec![data])
            }
        }
        process_recursive(&mut self.processors, context, data)
    }
}

fn build_iterative_pipeline(num_processors: usize) -> Pipeline<u32> {
    let mut pipeline = Pipeline::new(Arc::new(StreamerContext::new(CancellationToken::new())));
    for _ in 0..num_processors {
        pipeline = pipeline.add_processor(CounterProcessor);
    }
    pipeline
}

fn build_recursive_pipeline(num_processors: usize) -> RecursivePipeline<u32> {
    let mut pipeline = RecursivePipeline::new();
    for _ in 0..num_processors {
        pipeline = pipeline.add_processor(CounterProcessor);
    }
    pipeline
}

fn pipeline_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Pipeline Comparison");
    let processor_counts = [3, 10];

    for count in processor_counts.iter() {
        group.bench_with_input(
            format!("Iterative - {count} processors"),
            count,
            |b, &num_processors| {
                b.iter_with_setup(
                    || build_iterative_pipeline(num_processors),
                    |pipeline| {
                        let mut result = 0;
                        let mut output = |res: Result<u32, PipelineError>| {
                            if let Ok(val) = res {
                                result = val;
                            }
                        };
                        pipeline
                            .run(std::hint::black_box(vec![Ok(0)].into_iter()), &mut output)
                            .unwrap();
                        assert_eq!(result, num_processors as u32);
                    },
                )
            },
        );

        group.bench_with_input(
            format!("Recursive - {count} processors"),
            count,
            |b, &num_processors| {
                b.iter_with_setup(
                    || {
                        (
                            build_recursive_pipeline(num_processors),
                            Arc::new(StreamerContext::new(CancellationToken::new())),
                        )
                    },
                    |(mut pipeline, context)| {
                        let result = pipeline.process(&context, std::hint::black_box(0)).unwrap();
                        assert_eq!(result, vec![num_processors as u32]);
                    },
                )
            },
        );
    }

    group.finish();
}

criterion_group!(benches, pipeline_benchmark);
criterion_main!(benches);
