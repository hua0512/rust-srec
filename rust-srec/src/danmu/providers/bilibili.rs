use crate::danmu::{DanmuMessage, DanmuProvider};
use anyhow::Result;

pub struct BilibiliDanmuProvider {
    _url: String,
}

impl BilibiliDanmuProvider {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            _url: url.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl DanmuProvider for BilibiliDanmuProvider {
    async fn connect(&mut self) -> Result<()> {
        // TODO: Implement Bilibili WebSocket connection
        Ok(())
    }

    async fn next_message(&mut self) -> Option<Result<DanmuMessage>> {
        // TODO: Implement Bilibili message parsing
        None
    }

    async fn close(&mut self) -> Result<()> {
        // TODO: Implement Bilibili WebSocket disconnection
        Ok(())
    }
}