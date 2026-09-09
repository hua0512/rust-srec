use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

use crate::Result;
use crate::database::models::{
    ChannelType, DiscordChannelSettings, EmailChannelSettings, GotifyChannelSettings,
    NotificationChannelDbModel, TelegramChannelSettings, WebhookChannelSettings,
};
use crate::notification::channels::{
    self, ChannelConfig, DiscordChannel, EmailChannel, GotifyChannel, NotificationChannel,
    TelegramChannel, WebhookChannel,
};
use crate::notification::events::{NotificationPriority, canonicalize_subscription_event_name};

use super::{
    CircuitBreakerState, NotificationChannelInstance, NotificationChannelSource,
    NotificationService,
};

#[derive(Clone)]
pub(super) struct RuntimeChannel {
    pub(super) key: String,
    pub(super) db_channel_id: Option<String>,
    pub(super) display_name: String,
    pub(super) channel_type: String,
    pub(super) channel: Arc<dyn NotificationChannel>,
    pub(super) breaker: Arc<parking_lot::Mutex<CircuitBreakerState>>,
}

#[derive(Default)]
pub(super) struct ChannelRegistry {
    pub(super) channels: Vec<Arc<RuntimeChannel>>,
    pub(super) by_key: HashMap<String, Arc<RuntimeChannel>>,
    pub(super) subscriptions_by_event: HashMap<String, Vec<String>>,
}

impl ChannelRegistry {
    pub(super) fn insert(&mut self, channel: Arc<RuntimeChannel>) {
        self.by_key.insert(channel.key.clone(), channel.clone());
        self.channels.push(channel);
    }
}

impl NotificationService {
    /// Initialize channels from configuration.
    pub(super) fn init_channels(&self) {
        let mut registry = self.registry.write();
        *registry = ChannelRegistry::default();
        let mut used_keys: HashSet<String> = HashSet::new();

        for (idx, channel_config) in self.config.channels.iter().enumerate() {
            let channel: Arc<dyn NotificationChannel> = match channel_config {
                ChannelConfig::Discord(c) => Arc::new(DiscordChannel::new(c.clone())),
                ChannelConfig::Email(c) => Arc::new(EmailChannel::new(c.clone())),
                ChannelConfig::Gotify(c) => Arc::new(GotifyChannel::new(c.clone())),
                ChannelConfig::Telegram(c) => Arc::new(TelegramChannel::new(c.clone())),
                ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
            };

            if channel.is_enabled() {
                let base_key = match channel_config.instance_id() {
                    Some(id) => format!(
                        "config:{}:{}",
                        channel_config.channel_type(),
                        normalize_channel_key_part(id)
                    ),
                    None => format!("config:{}:{}", channel_config.channel_type(), idx),
                };
                let key = if used_keys.insert(base_key.clone()) {
                    base_key
                } else {
                    let disambiguated = format!("{}:{}", base_key, idx);
                    warn!(
                        "Duplicate notification channel key detected (base_key={}), using {}",
                        base_key, disambiguated
                    );
                    used_keys.insert(disambiguated.clone());
                    disambiguated
                };
                let display_name = channel_config
                    .display_name()
                    .unwrap_or(channel_config.channel_type())
                    .to_string();

                let runtime = Arc::new(RuntimeChannel {
                    key,
                    db_channel_id: None,
                    display_name,
                    channel_type: channel.channel_type().to_string(),
                    channel,
                    breaker: self.new_breaker(),
                });
                registry.insert(runtime);
                info!(
                    "Initialized notification channel: {}",
                    channel_config.channel_type()
                );
            }
        }

        info!(
            "Notification service initialized with {} channels",
            registry.channels.len()
        );
    }

    /// Add a channel dynamically.
    pub fn add_channel(&self, config: ChannelConfig) {
        let channel: Arc<dyn NotificationChannel> = match &config {
            ChannelConfig::Discord(c) => Arc::new(DiscordChannel::new(c.clone())),
            ChannelConfig::Email(c) => Arc::new(EmailChannel::new(c.clone())),
            ChannelConfig::Gotify(c) => Arc::new(GotifyChannel::new(c.clone())),
            ChannelConfig::Telegram(c) => Arc::new(TelegramChannel::new(c.clone())),
            ChannelConfig::Webhook(c) => Arc::new(WebhookChannel::new(c.clone())),
        };

        if channel.is_enabled() {
            let key = format!("dynamic:{}", Uuid::new_v4());
            let display_name = config
                .display_name()
                .unwrap_or(config.channel_type())
                .to_string();
            let runtime = Arc::new(RuntimeChannel {
                key,
                db_channel_id: None,
                display_name,
                channel_type: channel.channel_type().to_string(),
                channel,
                breaker: self.new_breaker(),
            });
            self.registry.write().insert(runtime);
            info!("Added notification channel: {}", config.channel_type());
        }
    }

    pub(super) fn new_breaker(&self) -> Arc<parking_lot::Mutex<CircuitBreakerState>> {
        Arc::new(parking_lot::Mutex::new(CircuitBreakerState::new(
            self.config.circuit_breaker_cooldown_secs,
        )))
    }

    pub async fn reload_from_db(&self) -> Result<()> {
        let Some(repo) = self.notification_repo.as_ref().cloned() else {
            return Ok(());
        };
        let _reload = self.reload_gate.lock().await;
        let db_channels = repo.list_channels().await?;
        let mut new_db_channels = Vec::new();
        let mut subscriptions_by_event: HashMap<String, Vec<String>> = HashMap::new();
        let mut migrations = Vec::new();
        for db_channel in db_channels {
            let runtime = match self.build_runtime_channel_from_db(&db_channel) {
                Ok(Some(channel)) => channel,
                Ok(None) => continue,
                Err(error) => {
                    warn!(channel_id = %db_channel.id, channel_type = %db_channel.channel_type,
                        %error, "Skipping invalid notification channel");
                    continue;
                }
            };
            for raw_event in repo.get_subscriptions_for_channel(&db_channel.id).await? {
                let Some(canonical) = canonicalize_subscription_event_name(&raw_event) else {
                    warn!(channel_id = %db_channel.id, event = %raw_event, "Skipping unknown notification subscription");
                    continue;
                };
                subscriptions_by_event
                    .entry(canonical.to_owned())
                    .or_default()
                    .push(db_channel.id.clone());
                if raw_event.trim() != canonical {
                    migrations.push((db_channel.id.clone(), raw_event, canonical));
                }
            }
            new_db_channels.push(runtime);
        }

        let (channel_count, subscription_count) = {
            let mut current = self.registry.write();
            let mut next = ChannelRegistry {
                subscriptions_by_event,
                ..ChannelRegistry::default()
            };
            // Read dynamic/config channels only at publication, so additions during IO survive.
            for channel in current
                .channels
                .iter()
                .filter(|channel| channel.db_channel_id.is_none())
            {
                next.insert(channel.clone());
            }
            for mut channel in new_db_channels {
                if let Some(previous) = current.by_key.get(&channel.key) {
                    // A still-loaded DB ID is the channel identity even when settings change.
                    // Removal ends this generation; pending work owns its older Arc separately.
                    Arc::make_mut(&mut channel).breaker = previous.breaker.clone();
                }
                next.insert(channel);
            }
            let counts = (next.channels.len(), next.subscriptions_by_event.len());
            *current = next;
            counts
        };

        // Migration is best-effort and starts only after all discovery reads succeeded.
        // Never remove the legacy subscription unless its canonical replacement was stored.
        for (channel_id, raw_event, canonical) in migrations {
            if let Err(error) = repo.subscribe(&channel_id, canonical).await {
                warn!(%channel_id, %error, "Failed to migrate notification subscription");
                continue;
            }
            if let Err(error) = repo.unsubscribe(&channel_id, &raw_event).await {
                warn!(%channel_id, %error, "Failed to remove migrated notification subscription");
            }
        }
        info!(
            channel_count,
            subscription_count, "Notification DB config loaded"
        );
        Ok(())
    }
    fn build_runtime_channel_from_db(
        &self,
        db_channel: &NotificationChannelDbModel,
    ) -> Result<Option<Arc<RuntimeChannel>>> {
        let settings_json: Value = serde_json::from_str(&db_channel.settings)?;
        let enabled = settings_json
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            return Ok(None);
        }

        let channel_type = ChannelType::parse(&db_channel.channel_type).ok_or_else(|| {
            crate::Error::Validation(format!(
                "Unsupported notification channel_type: {}",
                db_channel.channel_type
            ))
        })?;

        let min_priority = settings_json
            .get("min_priority")
            .and_then(|v| {
                // Accept integer (new format) or string (legacy format).
                if let Some(n) = v.as_u64() {
                    NotificationPriority::from_int(n as u8)
                } else {
                    v.as_str().and_then(parse_notification_priority)
                }
            })
            .unwrap_or(NotificationPriority::Normal);

        let locale = parse_channel_locale(&settings_json);

        let runtime_channel: Arc<dyn NotificationChannel> = match channel_type {
            ChannelType::Discord => {
                let settings: DiscordChannelSettings =
                    serde_json::from_value(settings_json.clone())?;
                Arc::new(DiscordChannel::new(channels::DiscordConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    webhook_url: settings.webhook_url,
                    username: settings.username,
                    avatar_url: settings.avatar_url,
                    min_priority,
                    locale: locale.clone(),
                }))
            }
            ChannelType::Email => {
                let settings: EmailChannelSettings = serde_json::from_value(settings_json.clone())?;
                Arc::new(EmailChannel::new(channels::EmailConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    smtp_host: settings.smtp_host,
                    smtp_port: settings.smtp_port,
                    smtp_username: if settings.username.is_empty() {
                        None
                    } else {
                        Some(settings.username)
                    },
                    smtp_password: if settings.password.is_empty() {
                        None
                    } else {
                        Some(settings.password)
                    },
                    use_tls: settings.use_tls,
                    from_address: settings.from_address,
                    to_addresses: settings.to_addresses,
                    min_priority,
                    locale: locale.clone(),
                }))
            }
            ChannelType::Telegram => {
                let settings: TelegramChannelSettings =
                    serde_json::from_value(settings_json.clone())?;
                Arc::new(TelegramChannel::new(channels::TelegramConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    bot_token: settings.bot_token,
                    chat_id: settings.chat_id,
                    parse_mode: settings_json
                        .get("parse_mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("HTML")
                        .to_string(),
                    min_priority,
                    locale: locale.clone(),
                }))
            }
            ChannelType::Webhook => {
                let settings: WebhookChannelSettings =
                    serde_json::from_value(settings_json.clone())?;

                let mut headers_vec = Vec::new();
                if let Some(headers) = settings.headers {
                    let mut keys: Vec<_> = headers.keys().cloned().collect();
                    keys.sort();
                    for k in keys {
                        if let Some(v) = headers.get(&k) {
                            headers_vec.push((k.clone(), v.clone()));
                        }
                    }
                }

                let auth = if let Some(auth_val) = settings.auth {
                    match serde_json::from_value::<channels::WebhookAuth>(auth_val.clone()) {
                        Ok(a) => Some(a),
                        Err(e) => {
                            warn!("Failed to parse webhook auth for channel: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                Arc::new(WebhookChannel::new(channels::WebhookConfig {
                    id: None,
                    name: None,
                    enabled: settings.enabled.unwrap_or(true),
                    url: settings.url,
                    method: settings.method,
                    headers: headers_vec,
                    auth,
                    min_priority,
                    locale: locale.clone(),
                    timeout_secs: settings.timeout_secs.unwrap_or(30),
                }))
            }
            ChannelType::Gotify => {
                let settings: GotifyChannelSettings =
                    serde_json::from_value(settings_json.clone())?;
                Arc::new(GotifyChannel::new(channels::GotifyConfig {
                    id: None,
                    name: None,
                    enabled: true,
                    server_url: settings.server_url,
                    app_token: settings.app_token,
                    min_priority,
                    locale: locale.clone(),
                    timeout_secs: settings.timeout_secs,
                }))
            }
        };

        if !runtime_channel.is_enabled() {
            return Ok(None);
        }

        Ok(Some(Arc::new(RuntimeChannel {
            key: db_channel.id.clone(),
            db_channel_id: Some(db_channel.id.clone()),
            display_name: db_channel.name.clone(),
            channel_type: db_channel.channel_type.clone(),
            channel: runtime_channel,
            breaker: self.new_breaker(),
        })))
    }

    /// List currently loaded channel instances (config + dynamic + DB).
    pub fn list_channel_instances(&self) -> Vec<NotificationChannelInstance> {
        self.registry
            .read()
            .channels
            .iter()
            .map(|c| NotificationChannelInstance {
                key: c.key.clone(),
                channel_id: c.db_channel_id.clone(),
                display_name: c.display_name.clone(),
                channel_type: c.channel_type.clone(),
                source: if c.db_channel_id.is_some() {
                    NotificationChannelSource::Database
                } else if c.key.starts_with("dynamic:") {
                    NotificationChannelSource::Dynamic
                } else {
                    NotificationChannelSource::Config
                },
            })
            .collect()
    }

    /// Run a connectivity/config test for a specific channel instance.
    pub async fn test_channel_instance(&self, key: &str) -> Result<()> {
        let channel = self
            .registry
            .read()
            .by_key
            .get(key)
            .cloned()
            .ok_or_else(|| crate::Error::NotFound {
                entity_type: "NotificationChannelInstance".to_string(),
                id: key.to_string(),
            })?;
        channel.channel.test().await
    }
}

/// Read a channel's configured language out of its settings blob.
///
/// Rides in the same JSON as `min_priority`, so the channel has no column of its own. Blank and
/// whitespace-only values mean "follow the locale `crate::i18n::set_locale` applied at startup"
/// and become `None`, rather than reaching `rust_i18n` as a locale with no YAML behind it.
pub(super) fn parse_channel_locale(settings: &Value) -> Option<String> {
    settings
        .get("locale")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_notification_priority(value: &str) -> Option<NotificationPriority> {
    // Try parsing as integer first (new format).
    if let Ok(int_val) = value.trim().parse::<u8>() {
        return NotificationPriority::from_int(int_val);
    }
    // Fall back to string labels (legacy format).
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some(NotificationPriority::Low),
        "normal" => Some(NotificationPriority::Normal),
        "high" => Some(NotificationPriority::High),
        "critical" => Some(NotificationPriority::Critical),
        _ => None,
    }
}

fn normalize_channel_key_part(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "_".to_string();
    }

    trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
