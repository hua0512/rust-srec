//! Saved proxies and the route each request takes.
//!
//! Pure value types, route resolution, and environment scrubbing live in
//! [`crate::domain::proxy`]; this module provides the database-backed
//! [`ProxyService`], [`ServiceRoutes`], HTTP probing, and legacy-config
//! parsing.

pub(crate) mod legacy;
pub(crate) mod platforms;
mod probe;
mod service;
mod service_routes;

pub use crate::domain::proxy::{
    AccountReference, NamedReference, OutputScrubber, ProxyEndpoint, ProxyEntry, ProxyError,
    ProxyName, ProxyReferences, ProxyRoute, ProxyRouteResolver, ProxyTarget, ResolvedRoute,
    RouteKey, RouteKind, RouteSource, SystemProxy, SystemProxySummary, TemplateReference,
    canonical, materialize, name_key, resolve, set_tool_environment, upload_target,
};
pub use platforms::{
    build_platforms_client, danmu_proxy_for, danmu_proxy_for_route, to_platforms_endpoint,
    to_platforms_target,
};
pub use probe::{PROBE_TIMEOUT, ProbeErrorKind, ProbeOutcome, platform_homepage, probe};
pub use service::{ProxyService, RouteScope};
pub use service_routes::ServiceRoutes;
