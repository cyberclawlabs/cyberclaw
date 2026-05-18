//! GitHub Connector 额外测试

#[cfg(test)]
mod extended_tests {
    use super::super::*;
    use std::sync::Arc;

    #[test]
    fn test_credentials_variants() {
        // PAT
        let pat = Credentials::PersonalAccessToken("token123".to_string());
        let json = serde_json::to_value(&pat).unwrap();
        assert!(json.is_object());

        // OAuth
        let oauth = Credentials::OAuth {
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(chrono::Utc::now()),
        };
        let json = serde_json::to_value(&oauth).unwrap();
        assert!(json.is_object());
    }

    #[test]
    fn test_is_expired_logic() {
        // PAT never expires
        let pat = Credentials::PersonalAccessToken("token".to_string());
        assert!(!is_expired(&pat));

        // OAuth with no expiry
        let oauth_no_exp = Credentials::OAuth {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
        };
        assert!(!is_expired(&oauth_no_exp));

        // OAuth expired
        let oauth_expired = Credentials::OAuth {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        };
        assert!(is_expired(&oauth_expired));

        // OAuth valid
        let oauth_valid = Credentials::OAuth {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        };
        assert!(!is_expired(&oauth_valid));
    }

    #[tokio::test]
    async fn test_rate_limiter_multiple_acquires() {
        let limiter = RateLimiter::new(120); // 120 req/min

        // 连续获取多个许可应该成功
        for _ in 0..5 {
            limiter.acquire().await.unwrap();
        }
    }

    #[test]
    fn test_github_auth_variants() {
        // From token
        let auth1 = GitHubAuth::from_token("pat_token".to_string());
        let debug_str = format!("{:?}", auth1);
        assert!(!debug_str.is_empty());

        // With OAuth
        let auth2 = GitHubAuth::new("client_id".to_string(), "client_secret".to_string());
        let debug_str = format!("{:?}", auth2);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn test_connector_capability_metadata() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);

        // 检查每个能力的元数据
        for cap in connector.capabilities() {
            assert!(!cap.id.is_empty());
            assert!(!cap.title.is_empty());
            assert!(cap.description.is_some());
            assert!(!cap.input_schema.is_empty());
            assert!(!cap.output_schema.is_empty());
            assert!(!cap.effects.is_empty());
        }
    }

    #[test]
    fn test_connector_capabilities_risk_levels() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);

        let caps = connector.capabilities();

        // 验证特定能力的风险级别
        let create_issue = caps.iter().find(|c| c.id == "github.create_issue").unwrap();
        assert_eq!(create_issue.risk, RiskLevel::Medium);

        let create_pr = caps.iter().find(|c| c.id == "github.create_pr").unwrap();
        assert_eq!(create_pr.risk, RiskLevel::High);

        let list_repos = caps.iter().find(|c| c.id == "github.list_repos").unwrap();
        assert_eq!(list_repos.risk, RiskLevel::Low);

        let search_code = caps.iter().find(|c| c.id == "github.search_code").unwrap();
        assert_eq!(search_code.risk, RiskLevel::Low);

        let review_code = caps.iter().find(|c| c.id == "github.review_code").unwrap();
        assert_eq!(review_code.risk, RiskLevel::Medium);
    }

    #[tokio::test]
    async fn test_github_auth_cached_token() {
        let auth = GitHubAuth::from_token("cached_token".to_string());

        // 第一次认证
        let creds1 = auth.authenticate().await.unwrap();

        // 第二次应该使用缓存
        let creds2 = auth.authenticate().await.unwrap();

        // 两次都应该返回相同的 token
        match (creds1, creds2) {
            (Credentials::PersonalAccessToken(t1), Credentials::PersonalAccessToken(t2)) => {
                assert_eq!(t1, t2);
                assert_eq!(t1, "cached_token");
            }
            _ => panic!("Expected PAT credentials"),
        }
    }

    #[test]
    fn test_connector_id_format() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);

        assert_eq!(connector.id().as_str(), "github");
    }

    #[test]
    fn test_connector_runtime_type() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);

        // ConnectorRuntime 不实现 PartialEq，所以使用 matches!
        assert!(matches!(
            connector.runtime(),
            cyberclaw_core::manifests::ConnectorRuntime::Native
        ));
    }

    #[test]
    fn test_capability_input_schemas_completeness() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);
        let capabilities = connector.capabilities();

        // 验证每个能力的输入架构包含必需字段
        let create_issue = capabilities
            .iter()
            .find(|c| c.id == "github.create_issue")
            .unwrap();
        assert!(create_issue.input_schema.contains("owner"));
        assert!(create_issue.input_schema.contains("repo"));
        assert!(create_issue.input_schema.contains("title"));

        let create_pr = capabilities
            .iter()
            .find(|c| c.id == "github.create_pr")
            .unwrap();
        assert!(create_pr.input_schema.contains("head"));
        assert!(create_pr.input_schema.contains("base"));
    }

    #[test]
    fn test_capability_output_schemas_completeness() {
        let auth = Arc::new(GitHubAuth::from_token("test".to_string()));
        let connector = GitHubConnector::new(auth);
        let capabilities = connector.capabilities();

        // 验证输出架构
        let create_issue = capabilities
            .iter()
            .find(|c| c.id == "github.create_issue")
            .unwrap();
        assert!(create_issue.output_schema.contains("number"));
        assert!(create_issue.output_schema.contains("html_url"));

        let search_code = capabilities
            .iter()
            .find(|c| c.id == "github.search_code")
            .unwrap();
        assert!(search_code.output_schema.contains("total_count"));
        assert!(search_code.output_schema.contains("items"));
    }
}
