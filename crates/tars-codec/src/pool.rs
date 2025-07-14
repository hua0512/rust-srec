use crate::{TarsError, TarsMessage, de::TarsDeserializer, ser::TarsSerializer};
use bytes::{Bytes, BytesMut};
use std::sync::Mutex;

/// A thread-safe object pool for reusing TARS codec components
/// Reduces allocation overhead in high-throughput scenarios
pub struct TarsCodecPool {
    serializers: Mutex<Vec<TarsSerializer>>,
    deserializers: Mutex<Vec<TarsDeserializer>>,
    #[allow(dead_code)]
    message_buffers: Mutex<Vec<TarsMessage>>,
    byte_buffers: Mutex<Vec<BytesMut>>,
}

impl TarsCodecPool {
    /// Create a new codec pool with initial capacity
    pub fn new(initial_capacity: usize) -> Self {
        let mut serializers = Vec::with_capacity(initial_capacity);
        let deserializers = Vec::with_capacity(initial_capacity);
        let mut message_buffers = Vec::with_capacity(initial_capacity);
        let mut byte_buffers = Vec::with_capacity(initial_capacity);

        // Pre-populate with reusable objects
        for _ in 0..initial_capacity {
            serializers.push(TarsSerializer::new());
            message_buffers.push(Self::create_empty_message());
            byte_buffers.push(BytesMut::with_capacity(1024)); // Default buffer size
        }

        Self {
            serializers: Mutex::new(serializers),
            deserializers: Mutex::new(deserializers),
            message_buffers: Mutex::new(message_buffers),
            byte_buffers: Mutex::new(byte_buffers),
        }
    }

    /// Get a pooled serializer, or create a new one if pool is empty
    pub fn get_serializer(&self) -> PooledSerializer {
        let serializer = self.serializers.lock().unwrap().pop().unwrap_or_default();
        PooledSerializer {
            inner: Some(serializer),
            pool: self,
        }
    }

    /// Get a pooled deserializer for the given bytes
    pub fn get_deserializer(&self, bytes: Bytes) -> PooledDeserializer {
        let deserializer = self
            .deserializers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| TarsDeserializer::new(bytes.clone()));

        // Reset the deserializer with new bytes
        let mut de = deserializer;
        de.reset(bytes);

        PooledDeserializer {
            inner: Some(de),
            pool: self,
        }
    }

    /// Get a pooled byte buffer for encoding
    pub fn get_byte_buffer(&self, estimated_size: usize) -> PooledByteBuffer {
        let mut buffer = self
            .byte_buffers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| BytesMut::with_capacity(estimated_size));

        // Ensure buffer has sufficient capacity
        if buffer.capacity() < estimated_size {
            buffer.reserve(estimated_size - buffer.capacity());
        }
        buffer.clear(); // Reset for reuse

        PooledByteBuffer {
            inner: Some(buffer),
            pool: self,
        }
    }

    /// Return a serializer to the pool
    fn return_serializer(&self, mut serializer: TarsSerializer) {
        serializer.reset(); // Clear internal state
        let mut pool = self.serializers.lock().unwrap();
        if pool.len() < 16 {
            // Limit pool size to prevent unbounded growth
            pool.push(serializer);
        }
    }

    /// Return a deserializer to the pool  
    fn return_deserializer(&self, deserializer: TarsDeserializer) {
        let mut pool = self.deserializers.lock().unwrap();
        if pool.len() < 16 {
            // Limit pool size
            pool.push(deserializer);
        }
    }

    /// Return a byte buffer to the pool
    fn return_byte_buffer(&self, mut buffer: BytesMut) {
        buffer.clear(); // Reset content but keep capacity
        let mut pool = self.byte_buffers.lock().unwrap();
        if pool.len() < 16 && buffer.capacity() <= 16384 {
            // Limit pool size and buffer size
            pool.push(buffer);
        }
    }

    fn create_empty_message() -> TarsMessage {
        use crate::{TarsMessage, TarsRequestHeader};
        use rustc_hash::FxHashMap;

        TarsMessage {
            header: TarsRequestHeader {
                version: 0,
                packet_type: 0,
                message_type: 0,
                request_id: 0,
                servant_name: String::new(),
                func_name: String::new(),
                timeout: 0,
                context: FxHashMap::default(),
                status: FxHashMap::default(),
            },
            body: FxHashMap::default(),
        }
    }
}

impl Default for TarsCodecPool {
    fn default() -> Self {
        Self::new(4) // Default capacity of 4
    }
}

/// A pooled serializer that returns itself to the pool when dropped
pub struct PooledSerializer<'a> {
    inner: Option<TarsSerializer>,
    pool: &'a TarsCodecPool,
}

impl<'a> PooledSerializer<'a> {
    /// Encode a message using the pooled serializer (zero-copy reference)
    pub fn encode_message(&mut self, message: &TarsMessage) -> Result<&BytesMut, TarsError> {
        let serializer = self.inner.as_mut().unwrap();
        serializer.encode_message(message)
    }

    /// Encode a message using the pooled serializer (consumes message)
    pub fn encode_message_owned(&mut self, message: TarsMessage) -> Result<BytesMut, TarsError> {
        let serializer = self.inner.as_mut().unwrap();
        serializer.encode_message_owned(message)
    }

    /// Get the internal serializer (for advanced usage)
    pub fn serializer(&mut self) -> &mut TarsSerializer {
        self.inner.as_mut().unwrap()
    }
}

impl<'a> Drop for PooledSerializer<'a> {
    fn drop(&mut self) {
        if let Some(serializer) = self.inner.take() {
            self.pool.return_serializer(serializer);
        }
    }
}

/// A pooled deserializer that returns itself to the pool when dropped
pub struct PooledDeserializer<'a> {
    inner: Option<TarsDeserializer>,
    pool: &'a TarsCodecPool,
}

impl<'a> PooledDeserializer<'a> {
    /// Decode a message using the pooled deserializer
    pub fn decode_message(&mut self) -> Result<TarsMessage, TarsError> {
        let deserializer = self.inner.as_mut().unwrap();
        deserializer.read_message()
    }

    /// Get the internal deserializer (for advanced usage)
    pub fn deserializer(&mut self) -> &mut TarsDeserializer {
        self.inner.as_mut().unwrap()
    }
}

impl<'a> Drop for PooledDeserializer<'a> {
    fn drop(&mut self) {
        if let Some(deserializer) = self.inner.take() {
            self.pool.return_deserializer(deserializer);
        }
    }
}

/// A pooled byte buffer that returns itself to the pool when dropped
pub struct PooledByteBuffer<'a> {
    inner: Option<BytesMut>,
    pool: &'a TarsCodecPool,
}

impl<'a> PooledByteBuffer<'a> {
    /// Get mutable access to the buffer
    pub fn buffer(&mut self) -> &mut BytesMut {
        self.inner.as_mut().unwrap()
    }

    /// Convert to the underlying BytesMut (consumes the wrapper)
    pub fn into_bytes(mut self) -> BytesMut {
        self.inner.take().unwrap()
    }
}

impl<'a> Drop for PooledByteBuffer<'a> {
    fn drop(&mut self) {
        if let Some(buffer) = self.inner.take() {
            self.pool.return_byte_buffer(buffer);
        }
    }
}

/// High-level pooled encoding function
impl TarsCodecPool {
    /// Encode a message using a pooled serializer and buffer (most efficient)
    pub fn encode_pooled(&self, message: &TarsMessage) -> Result<BytesMut, TarsError> {
        let estimated_size = crate::estimate_message_size(message);
        let mut buffer = self.get_byte_buffer(estimated_size);
        let mut serializer = self.get_serializer();

        // Use the pooled objects to encode directly to buffer (no intermediate cloning)
        serializer
            .serializer()
            .encode_message_to_buffer(message, buffer.buffer())?;
        Ok(buffer.into_bytes())
    }

    /// Encode a message using a pooled serializer and buffer (consumes message)
    pub fn encode_pooled_owned(&self, message: TarsMessage) -> Result<BytesMut, TarsError> {
        self.encode_pooled(&message)
    }

    /// Decode a message using a pooled deserializer
    pub fn decode_pooled(&self, bytes: Bytes) -> Result<TarsMessage, TarsError> {
        let mut deserializer = self.get_deserializer(bytes);
        deserializer.decode_message()
    }
}
