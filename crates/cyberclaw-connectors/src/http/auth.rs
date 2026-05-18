//! HTTP authentication strategies for HttpConnector

use reqwest::RequestBuilder;

/// Authentication strategy for HTTP requests
#[derive(Debug, Clone)]
pub enum AuthStrategy {
    /// No authentication
    None,
    /// Bearer token authentication (`Authorization: Bearer <token>`)
    Bearer(String),
    /// HTTP Basic authentication
    Basic { username: String, password: String },
    /// Custom header authentication
    CustomHeader { name: String, value: String },
}

impl AuthStrategy {
    /// Apply this authentication strategy to a request builder, returning the modified builder
    pub fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self {
            AuthStrategy::None => request,
            AuthStrategy::Bearer(token) => {
                request.header("Authorization", format!("Bearer {}", token))
            }
            AuthStrategy::Basic { username, password } => {
                request.basic_auth(username, Some(password))
            }
            AuthStrategy::CustomHeader { name, value } => {
                request.header(name.as_str(), value.as_str())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_strategy_none_is_debug() {
        let auth = AuthStrategy::None;
        let s = format!("{:?}", auth);
        assert!(s.contains("None"));
    }

    #[test]
    fn test_auth_strategy_bearer_stores_token() {
        let auth = AuthStrategy::Bearer("my-token".to_string());
        if let AuthStrategy::Bearer(token) = &auth {
            assert_eq!(token, "my-token");
        } else {
            panic!("Expected Bearer variant");
        }
    }

    #[test]
    fn test_auth_strategy_basic_stores_credentials() {
        let auth = AuthStrategy::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        if let AuthStrategy::Basic { username, password } = &auth {
            assert_eq!(username, "user");
            assert_eq!(password, "pass");
        } else {
            panic!("Expected Basic variant");
        }
    }

    #[test]
    fn test_auth_strategy_custom_header_stores_values() {
        let auth = AuthStrategy::CustomHeader {
            name: "X-Api-Key".to_string(),
            value: "secret123".to_string(),
        };
        if let AuthStrategy::CustomHeader { name, value } = &auth {
            assert_eq!(name, "X-Api-Key");
            assert_eq!(value, "secret123");
        } else {
            panic!("Expected CustomHeader variant");
        }
    }
}
