//! Saved proxies and the route each request takes.
//!
//! A proxy entry is a named, reusable address with an optional login. Global,
//! platform, template and streamer settings and each account choose a
//! [`ProxyRoute`]: inherit, direct, the environment's proxy, or an entry.
//! Resolution turns the applicable routes into a [`ResolvedRoute`], which
//! carries the [`ProxyTarget`] every client, download engine and helper
//! process connects by and the [`RouteKey`] platform throttles are kept per.
//!
//! An account's own route wins over the scopes of the operation using it. An
//! account that inherits follows the operation's route, and management of the
//! account itself (checks, refreshes, sign-in) follows its platform, then the
//! global route. A route naming an entry that no longer exists fails rather
//! than connecting directly.
//!
//! Notification channels, browser push and upload tools keep their own
//! clients, which follow the environment.

mod endpoint;
mod error;
pub(crate) mod legacy;
mod probe;
mod resolve;
mod route;
mod service;
mod system;

pub use endpoint::{ProxyEndpoint, canonical};
pub use error::{AccountReference, NamedReference, ProxyError, ProxyReferences, TemplateReference};
pub use platforms_parser::proxy::ProxyTarget;
pub use probe::{PROBE_TIMEOUT, ProbeErrorKind, ProbeOutcome, platform_homepage, probe};
pub use resolve::{ProxyEntry, materialize, resolve};
pub use route::{ProxyName, ProxyRoute, ResolvedRoute, RouteKey, RouteKind, RouteSource, name_key};
pub use service::{ProxyService, RouteScope};
pub use system::{SystemProxy, SystemProxySummary};
