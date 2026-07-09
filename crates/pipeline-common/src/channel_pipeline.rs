use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tracing::error;

use crate::{Pipeline, PipelineError};

const DEFAULT_CHANNEL_CAPACITY: usize = 32;
const DEFAULT_MAX_BATCH_ITEMS: usize = 64;

#[derive(Clone, Copy)]
enum ChannelMode<T> {
    Items,
    Bytes {
        budget: u32,
        item_size: fn(&T) -> usize,
    },
}

#[derive(Clone, Copy)]
pub struct ChannelSpec<T> {
    capacity: usize,
    max_batch_items: usize,
    mode: ChannelMode<T>,
}

impl<T> ChannelSpec<T> {
    pub fn items(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            max_batch_items: DEFAULT_MAX_BATCH_ITEMS,
            mode: ChannelMode::Items,
        }
    }

    pub fn bytes(budget: usize, item_size: fn(&T) -> usize) -> Self {
        let max_budget = Semaphore::MAX_PERMITS.min(u32::MAX as usize);
        Self {
            capacity: DEFAULT_CHANNEL_CAPACITY,
            max_batch_items: DEFAULT_MAX_BATCH_ITEMS,
            mode: ChannelMode::Bytes {
                budget: budget.clamp(1, max_budget) as u32,
                item_size,
            },
        }
    }

    pub fn with_item_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self
    }

    pub fn with_max_batch_items(mut self, max_batch_items: usize) -> Self {
        self.max_batch_items = max_batch_items.max(1);
        self
    }

    fn batch_items(&self) -> usize {
        self.max_batch_items.min(self.capacity)
    }

    fn byte_limiter(&self) -> Option<ByteLimiter<T>> {
        match self.mode {
            ChannelMode::Items => None,
            ChannelMode::Bytes { budget, item_size } => Some(ByteLimiter::new(budget, item_size)),
        }
    }
}

impl<T> Default for ChannelSpec<T> {
    fn default() -> Self {
        Self::items(DEFAULT_CHANNEL_CAPACITY)
    }
}

struct ByteLimiter<T> {
    semaphore: Arc<Semaphore>,
    budget: u32,
    item_size: fn(&T) -> usize,
}

impl<T> Clone for ByteLimiter<T> {
    fn clone(&self) -> Self {
        Self {
            semaphore: Arc::clone(&self.semaphore),
            budget: self.budget,
            item_size: self.item_size,
        }
    }
}

impl<T> ByteLimiter<T> {
    fn new(budget: u32, item_size: fn(&T) -> usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(budget as usize)),
            budget,
            item_size,
        }
    }

    fn permits_for(&self, item: &Result<T, PipelineError>) -> u32 {
        item.as_ref().map_or(0, |item| {
            (self.item_size)(item).min(self.budget as usize) as u32
        })
    }

    async fn acquire(
        &self,
        permits: u32,
    ) -> Result<Option<OwnedSemaphorePermit>, tokio::sync::AcquireError> {
        if permits == 0 {
            return Ok(None);
        }

        Arc::clone(&self.semaphore)
            .acquire_many_owned(permits)
            .await
            .map(Some)
    }
}

struct BudgetedMessage<T> {
    item: Result<T, PipelineError>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl<T> BudgetedMessage<T> {
    fn into_item(self) -> Result<T, PipelineError> {
        self.item
    }
}

pub struct PipelineSender<T> {
    tx: mpsc::Sender<BudgetedMessage<T>>,
    limiter: Option<ByteLimiter<T>>,
}

impl<T> Clone for PipelineSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            limiter: self.limiter.clone(),
        }
    }
}

impl<T> PipelineSender<T> {
    fn new(tx: mpsc::Sender<BudgetedMessage<T>>, limiter: Option<ByteLimiter<T>>) -> Self {
        Self { tx, limiter }
    }

    pub async fn send(
        &self,
        item: Result<T, PipelineError>,
    ) -> Result<(), mpsc::error::SendError<Result<T, PipelineError>>> {
        let permit = if let Some(limiter) = &self.limiter {
            match limiter.acquire(limiter.permits_for(&item)).await {
                Ok(permit) => permit,
                Err(_) => return Err(mpsc::error::SendError(item)),
            }
        } else {
            None
        };

        self.tx
            .send(BudgetedMessage {
                item,
                _permit: permit,
            })
            .await
            .map_err(|error| mpsc::error::SendError(error.0.item))
    }
}

struct OutputBatch<T> {
    items: Vec<Result<T, PipelineError>>,
    permit: Option<OwnedSemaphorePermit>,
}

enum ReceiverKind<T> {
    Batched(mpsc::Receiver<OutputBatch<T>>),
    Items(mpsc::Receiver<Result<T, PipelineError>>),
}

pub struct PipelineReceiver<T> {
    kind: ReceiverKind<T>,
    pending: std::vec::IntoIter<Result<T, PipelineError>>,
    pending_permit: Option<OwnedSemaphorePermit>,
}

impl<T> PipelineReceiver<T> {
    fn batched(rx: mpsc::Receiver<OutputBatch<T>>) -> Self {
        Self {
            kind: ReceiverKind::Batched(rx),
            pending: Vec::new().into_iter(),
            pending_permit: None,
        }
    }

    pub fn from_items(rx: mpsc::Receiver<Result<T, PipelineError>>) -> Self {
        Self {
            kind: ReceiverKind::Items(rx),
            pending: Vec::new().into_iter(),
            pending_permit: None,
        }
    }

    pub async fn recv(&mut self) -> Option<Result<T, PipelineError>> {
        loop {
            if let Some(item) = self.pending.next() {
                return Some(item);
            }
            self.pending_permit = None;

            match &mut self.kind {
                ReceiverKind::Batched(rx) => {
                    let batch = rx.recv().await?;
                    self.pending = batch.items.into_iter();
                    self.pending_permit = batch.permit;
                }
                ReceiverKind::Items(rx) => return rx.recv().await,
            }
        }
    }

    pub fn blocking_recv(&mut self) -> Option<Result<T, PipelineError>> {
        loop {
            if let Some(item) = self.pending.next() {
                return Some(item);
            }
            self.pending_permit = None;

            match &mut self.kind {
                ReceiverKind::Batched(rx) => {
                    let batch = rx.blocking_recv()?;
                    self.pending = batch.items.into_iter();
                    self.pending_permit = batch.permit;
                }
                ReceiverKind::Items(rx) => return rx.blocking_recv(),
            }
        }
    }
}

impl<T> From<mpsc::Receiver<Result<T, PipelineError>>> for PipelineReceiver<T> {
    fn from(rx: mpsc::Receiver<Result<T, PipelineError>>) -> Self {
        Self::from_items(rx)
    }
}

pub struct SpawnedPipeline<T> {
    pub input_tx: PipelineSender<T>,
    pub output_rx: PipelineReceiver<T>,
    pub tasks: Vec<JoinHandle<Result<(), PipelineError>>>,
}

pub fn spawn_pipeline<T>(mut pipeline: Pipeline<T>, spec: ChannelSpec<T>) -> SpawnedPipeline<T>
where
    T: Send + 'static,
{
    let batch_items = spec.batch_items();
    let output_capacity = spec.capacity.div_ceil(batch_items).max(1);
    let input_limiter = spec.byte_limiter();
    let output_limiter = spec.byte_limiter();
    let runtime = Handle::current();
    let (input_tx, mut input_rx) = mpsc::channel::<BudgetedMessage<T>>(spec.capacity);
    let (output_tx, output_rx) = mpsc::channel::<OutputBatch<T>>(output_capacity);

    let task = tokio::task::spawn_blocking(move || {
        let mut inputs = Vec::with_capacity(batch_items);

        while let Some(first) = input_rx.blocking_recv() {
            inputs.push(first.into_item());
            while inputs.len() < batch_items {
                match input_rx.try_recv() {
                    Ok(item) => inputs.push(item.into_item()),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }

            let mut outputs = Vec::with_capacity(inputs.len());
            let process_result = pipeline.process_items(inputs.drain(..), &mut |item| {
                outputs.push(item);
            });

            if let Err(source) = process_result {
                if matches!(source, PipelineError::Cancelled) {
                    return Err(PipelineError::Cancelled);
                }

                let message = source.to_string();
                outputs.push(Err(source));
                send_output_items(
                    &output_tx,
                    output_limiter.as_ref(),
                    &runtime,
                    batch_items,
                    outputs,
                    "pipeline output",
                )?;
                error!(error = %message, "Pipeline processing failed");
                return Err(stage_process_error(message));
            }

            send_output_items(
                &output_tx,
                output_limiter.as_ref(),
                &runtime,
                batch_items,
                outputs,
                "pipeline output",
            )?;
        }

        let mut outputs = Vec::new();
        if let Err(source) = pipeline.finalize_processors(&mut |item| outputs.push(item)) {
            if matches!(source, PipelineError::Cancelled) {
                return Err(PipelineError::Cancelled);
            }

            let message = source.to_string();
            outputs.push(Err(source));
            send_output_items(
                &output_tx,
                output_limiter.as_ref(),
                &runtime,
                batch_items,
                outputs,
                "pipeline output during finish",
            )?;
            error!(error = %message, "Pipeline finalization failed");
            return Err(stage_finish_error(message));
        }

        send_output_items(
            &output_tx,
            output_limiter.as_ref(),
            &runtime,
            batch_items,
            outputs,
            "pipeline output during finish",
        )?;

        Ok(())
    });

    SpawnedPipeline {
        input_tx: PipelineSender::new(input_tx, input_limiter),
        output_rx: PipelineReceiver::batched(output_rx),
        tasks: vec![task],
    }
}

fn send_output_items<T>(
    output_tx: &mpsc::Sender<OutputBatch<T>>,
    limiter: Option<&ByteLimiter<T>>,
    runtime: &Handle,
    max_batch_items: usize,
    outputs: Vec<Result<T, PipelineError>>,
    channel_name: &'static str,
) -> Result<(), PipelineError> {
    let mut batch = Vec::with_capacity(outputs.len().min(max_batch_items));
    let mut batch_permits = 0u32;

    for output in outputs {
        let item_permits = limiter.map_or(0, |limiter| limiter.permits_for(&output));
        let byte_limit_reached = limiter.is_some()
            && !batch.is_empty()
            && batch_permits.saturating_add(item_permits)
                > limiter.map_or(u32::MAX, |limiter| limiter.budget);

        if batch.len() >= max_batch_items || byte_limit_reached {
            send_output_batch(
                output_tx,
                limiter,
                runtime,
                std::mem::take(&mut batch),
                batch_permits,
                channel_name,
            )?;
            batch_permits = 0;
        }

        batch_permits = batch_permits.saturating_add(item_permits);
        batch.push(output);
    }

    if !batch.is_empty() {
        send_output_batch(
            output_tx,
            limiter,
            runtime,
            batch,
            batch_permits,
            channel_name,
        )?;
    }

    Ok(())
}

fn send_output_batch<T>(
    output_tx: &mpsc::Sender<OutputBatch<T>>,
    limiter: Option<&ByteLimiter<T>>,
    runtime: &Handle,
    items: Vec<Result<T, PipelineError>>,
    permits: u32,
    channel_name: &'static str,
) -> Result<(), PipelineError> {
    let permit = if let Some(limiter) = limiter {
        runtime
            .block_on(limiter.acquire(permits))
            .map_err(|_| PipelineError::ChannelClosed(channel_name))?
    } else {
        None
    };

    output_tx
        .blocking_send(OutputBatch { items, permit })
        .map_err(|_| PipelineError::ChannelClosed(channel_name))
}

fn stage_process_error(message: String) -> PipelineError {
    PipelineError::StageProcess {
        stage: "Pipeline",
        source: Box::new(std::io::Error::other(message)),
    }
}

fn stage_finish_error(message: String) -> PipelineError {
    PipelineError::StageFinish {
        stage: "Pipeline",
        source: Box::new(std::io::Error::other(message)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::{CancellationToken, Processor, StreamerContext};
    use tokio::time::timeout;

    struct TestProcessor {
        processed_count: Arc<AtomicUsize>,
    }

    impl TestProcessor {
        fn new(processed_count: Arc<AtomicUsize>) -> Self {
            Self { processed_count }
        }
    }

    impl Processor<String> for TestProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: String,
            output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            self.processed_count.fetch_add(1, Ordering::SeqCst);
            output(format!("{input}-processed"))
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "TestProcessor"
        }
    }

    struct FailingProcessor;

    impl Processor<String> for FailingProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            _input: String,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "intentional failure",
            ))))
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "FailingProcessor"
        }
    }

    struct FinishFailingProcessor;

    impl Processor<String> for FinishFailingProcessor {
        fn process(
            &mut self,
            _context: &Arc<StreamerContext>,
            input: String,
            output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            output(input)
        }

        fn finish(
            &mut self,
            _context: &Arc<StreamerContext>,
            _output: &mut dyn FnMut(String) -> Result<(), PipelineError>,
        ) -> Result<(), PipelineError> {
            Err(PipelineError::Strategy(Box::new(std::io::Error::other(
                "intentional finish failure",
            ))))
        }

        fn name(&self) -> &'static str {
            "FinishFailingProcessor"
        }
    }

    #[tokio::test]
    async fn spawned_pipeline_batches_work_on_one_processing_task() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let pipeline = Pipeline::new(context).add_processor(TestProcessor::new(counter.clone()));
        let SpawnedPipeline {
            input_tx,
            mut output_rx,
            tasks,
        } = spawn_pipeline(pipeline, ChannelSpec::items(8).with_max_batch_items(4));

        assert_eq!(tasks.len(), 1);
        for item in ["one", "two", "three"] {
            input_tx.send(Ok(item.to_string())).await.unwrap();
        }
        drop(input_tx);

        let mut output = Vec::new();
        while let Some(item) = output_rx.recv().await {
            output.push(item.unwrap());
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        assert_eq!(
            output,
            ["one-processed", "two-processed", "three-processed"]
        );
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn byte_budget_allows_an_item_larger_than_the_budget() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let pipeline = Pipeline::new(context).add_processor(TestProcessor::new(counter));
        let SpawnedPipeline {
            input_tx,
            mut output_rx,
            tasks,
        } = spawn_pipeline(
            pipeline,
            ChannelSpec::bytes(4, String::len).with_max_batch_items(1),
        );

        timeout(
            Duration::from_secs(1),
            input_tx.send(Ok("oversized".to_string())),
        )
        .await
        .unwrap()
        .unwrap();
        drop(input_tx);

        let output = timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output, "oversized-processed");
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn byte_budget_backpressures_stalled_input_and_output_queues() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let pipeline =
            Pipeline::new(context).add_processor(TestProcessor::new(Arc::clone(&counter)));
        let SpawnedPipeline {
            input_tx,
            mut output_rx,
            tasks,
        } = spawn_pipeline(
            pipeline,
            ChannelSpec::bytes(4, String::len)
                .with_item_capacity(1)
                .with_max_batch_items(1),
        );

        input_tx.send(Ok("aaaa".to_string())).await.unwrap();
        timeout(Duration::from_secs(1), async {
            while counter.load(Ordering::SeqCst) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        input_tx.send(Ok("bbbb".to_string())).await.unwrap();
        timeout(Duration::from_secs(1), async {
            while counter.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        input_tx.send(Ok("cccc".to_string())).await.unwrap();

        let blocked_tx = input_tx.clone();
        let mut blocked_send =
            tokio::spawn(async move { blocked_tx.send(Ok("dddd".to_string())).await });
        assert!(
            timeout(Duration::from_millis(50), &mut blocked_send)
                .await
                .is_err()
        );

        assert_eq!(output_rx.recv().await.unwrap().unwrap(), "aaaa-processed");
        assert_eq!(output_rx.recv().await.unwrap().unwrap(), "bbbb-processed");
        timeout(Duration::from_secs(1), &mut blocked_send)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(input_tx);

        while output_rx.recv().await.is_some() {}
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn process_error_is_typed_on_channel_and_labeled_on_task() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let pipeline = Pipeline::new(context).add_processor(FailingProcessor);
        let SpawnedPipeline {
            input_tx,
            mut output_rx,
            tasks,
        } = spawn_pipeline(pipeline, ChannelSpec::items(2));

        input_tx.send(Ok("item".to_string())).await.unwrap();
        drop(input_tx);

        match output_rx.recv().await {
            Some(Err(PipelineError::Strategy(source))) => {
                assert_eq!(source.to_string(), "intentional failure");
            }
            other => panic!("expected strategy error, got {other:?}"),
        }
        match tasks.into_iter().next().unwrap().await.unwrap() {
            Err(PipelineError::StageProcess { stage, source }) => {
                assert_eq!(stage, "Pipeline");
                assert_eq!(source.to_string(), "intentional failure");
            }
            other => panic!("expected stage process error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn finish_error_is_typed_on_channel_and_labeled_on_task() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let pipeline = Pipeline::new(context).add_processor(FinishFailingProcessor);
        let SpawnedPipeline {
            input_tx,
            mut output_rx,
            tasks,
        } = spawn_pipeline(pipeline, ChannelSpec::items(2));

        input_tx.send(Ok("item".to_string())).await.unwrap();
        drop(input_tx);

        assert_eq!(output_rx.recv().await.unwrap().unwrap(), "item");
        match output_rx.recv().await {
            Some(Err(PipelineError::Strategy(source))) => {
                assert_eq!(source.to_string(), "intentional finish failure");
            }
            other => panic!("expected strategy error, got {other:?}"),
        }
        match tasks.into_iter().next().unwrap().await.unwrap() {
            Err(PipelineError::StageFinish { stage, source }) => {
                assert_eq!(stage, "Pipeline");
                assert_eq!(source.to_string(), "intentional finish failure");
            }
            other => panic!("expected stage finish error, got {other:?}"),
        }
    }
}
