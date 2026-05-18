//! HTTP/REST Connector module
//!
//! Provides `HttpConnector` for integrating external REST API services
//! as CyberClaw capabilities. Each configured endpoint becomes one capability.

pub mod auth;
pub mod connector;

pub use auth::AuthStrategy;
pub use connector::{HttpConnector, HttpConnectorConfig, HttpConnectorResponse, HttpEndpoint};
