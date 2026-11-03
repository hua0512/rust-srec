//! Which danmu provider and room a stream page's chat comes from.

use std::collections::HashMap;
use std::sync::Arc;

use platforms_parser::danmaku::{
    ConnectionConfig, DanmakuError, DanmuProvider, DanmuProxy, ProviderRegistry,
};

/// A provider and the room and settings to connect it with.
pub(crate) struct DanmuRoom {
    pub provider: Arc<dyn DanmuProvider>,
    pub room_id: String,
    pub config: ConnectionConfig,
}

/// Resolve the danmu room of the stream page `url`.
///
/// Several platforms join chat by an id the page URL does not carry, so the
/// extractor's `extras` take precedence over the URL where the platform
/// provides one.
pub(crate) fn resolve_room(
    providers: &ProviderRegistry,
    url: &str,
    cookies: Option<String>,
    extras: Option<HashMap<String, String>>,
    proxy: Option<DanmuProxy>,
) -> Result<DanmuRoom, DanmakuError> {
    let provider = providers
        .get_by_url(url)
        .ok_or_else(|| DanmakuError::connection(format!("No danmu provider for URL: {url}")))?;
    let extra = |keys: &[&str]| {
        let extras = extras.as_ref()?;
        keys.iter().find_map(|key| extras.get(*key)).cloned()
    };
    let room_id = match provider.platform() {
        "huya" => extra(&["presenter_uid"]),
        "douyin" => extra(&["id_str"]),
        "douyu" => extra(&["rid"]),
        // The chat path uses the bj id; chat host and FTK arrive in the extras.
        "soop" => extra(&["bjid", "channel_id"]),
        // Bigo enters by studio room id, not the site id in the URL.
        "bigo" => extra(&["room_id"]),
        // TikTok joins by numeric webcast room id; the URL carries only the
        // @handle, which the provider can resolve itself.
        "tiktok" => extra(&["room_id"]),
        _ => None,
    }
    .or_else(|| provider.extract_room_id(url))
    .ok_or_else(|| {
        DanmakuError::connection(format!("Could not extract room ID from URL: {url}"))
    })?;
    let mut config = ConnectionConfig::with_cookies(cookies).with_proxy(proxy);
    if let Some(extras) = extras {
        config = config.with_extras(extras);
    }
    Ok(DanmuRoom {
        provider,
        room_id,
        config,
    })
}
