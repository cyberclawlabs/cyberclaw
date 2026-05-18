//! JWT 认证中间件
//!
//! 提供基于 JWT 的 API 认证保护

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use cyberclaw_core::ids::{TenantId, UserId};
use cyberclaw_core::users::UsersConfig;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;

use crate::{error::ApiError, state::AppState};

/// JWT Claims 结构
///
/// `sub` is the typed [`UserId`]. `UserId` serializes as a plain string, so
/// tokens issued with the previous `String`-based shape remain parseable.
///
/// Sprint 20 Phase 1 (multi-tenant migration, ADR-0001):
/// `tenant` is the optional [`TenantId`] this caller belongs to.
/// `serde(default)` means tokens issued before Phase 1 (without the
/// claim) deserialize cleanly with `tenant = None`. The single-tenant
/// dispatch path is unchanged until Phase 2/3 of the migration wire
/// the field through to the storage layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 主体 (用户ID)
    pub sub: UserId,
    /// Optional tenant scope. `None` = system / single-tenant context.
    /// Phase 1 = present in claims but not enforced anywhere yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantId>,
    /// 过期时间 (Unix 时间戳)
    pub exp: i64,
    /// 签发时间 (Unix 时间戳)
    pub iat: i64,
}

impl Claims {
    /// Borrow the typed user id.
    pub fn user_id(&self) -> &UserId {
        &self.sub
    }

    /// Borrow the tenant id when this caller is tenant-scoped.
    /// Returns `None` for system actors and pre-Phase-1 legacy tokens.
    pub fn tenant(&self) -> Option<&TenantId> {
        self.tenant.as_ref()
    }
}

/// JWT 认证中间件
///
/// 从 Authorization header 中提取 Bearer token,
/// 验证签名和过期时间,并将 Claims 注入到请求 Extension 中
pub async fn jwt_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // 从 Authorization header 提取 token
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("Missing authorization header".to_string()))?;

    // 检查 Bearer 前缀
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::Unauthorized("Invalid authorization format".to_string()))?;

    // 验证 JWT
    let claims = verify_jwt(token, state.jwt_secret.as_bytes())?;

    // 将 Claims 注入到请求 Extension
    req.extensions_mut().insert(claims);

    // 继续处理请求
    Ok(next.run(req).await)
}

/// 验证 JWT token
///
/// # Arguments
///
/// * `token` - JWT token 字符串
/// * `secret` - JWT 签名密钥
///
/// # Returns
///
/// 成功返回 Claims,失败返回 ApiError::Unauthorized
pub fn verify_jwt(token: &str, secret: &[u8]) -> Result<Claims, ApiError> {
    let decoding_key = DecodingKey::from_secret(secret);
    let validation = Validation::new(Algorithm::HS256);

    decode::<Claims>(token, &decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(|e| ApiError::Unauthorized(format!("Invalid token: {}", e)))
}

/// Require that the caller's JWT `sub` maps to an [`OperatorRecord`] with
/// `role == "admin"`.
///
/// Sprint 8 L7 — server-side role enforcement. The frontend only hides admin
/// buttons, so without this guard a `viewer`-role operator could still reach
/// sensitive writes with a raw `curl`. This helper is called at the very top
/// of each sensitive handler (reviews approve/reject, `/admin/seed-demo`,
/// settings env/config PUT, skills install/install-remote/create, skills
/// uninstall).
///
/// Loads [`UsersConfig`] from disk per call. A cached decoder can land in
/// Sprint 10 when the operator registry moves behind a service. For now
/// "correct" beats "fast" — the admin lanes are low-traffic.
///
/// # Errors
///
/// - [`ApiError::InternalError`] if `users.toml` cannot be read.
/// - [`ApiError::Unauthorized`] if the JWT's `sub` no longer maps to any
///   operator record (the same 401 the rest of the admin surface returns).
/// - [`ApiError::Forbidden`] if the record exists but `role != "admin"`.
pub async fn require_admin(claims: &Claims) -> Result<(), ApiError> {
    let path = UsersConfig::default_path();
    let cfg = UsersConfig::load_from_path(&path).map_err(|e| {
        warn!(path = %path.display(), error = %e, "require_admin: users.toml load failed");
        ApiError::InternalError(format!("failed to load users config: {}", e))
    })?;
    let record = cfg.find(&claims.sub).ok_or_else(|| {
        warn!(user_id = %claims.sub, "require_admin: jwt references unknown operator");
        ApiError::Unauthorized("operator no longer exists".to_string())
    })?;
    if record.role != "admin" {
        warn!(
            user_id = %claims.sub,
            role = %record.role,
            "require_admin: non-admin role denied"
        );
        return Err(ApiError::Forbidden("admin role required".to_string()));
    }
    Ok(())
}

/// 生成 JWT token
///
/// # Arguments
///
/// * `user_id` - 用户ID (typed)
/// * `secret` - JWT 签名密钥
/// * `expires_in_secs` - 过期时间(秒)
pub fn generate_jwt(
    user_id: &UserId,
    secret: &[u8],
    expires_in_secs: i64,
) -> Result<String, ApiError> {
    generate_jwt_with_tenant(user_id, None, secret, expires_in_secs)
}

/// Multi-tenant variant of [`generate_jwt`]. `tenant` is `Some(...)` when
/// the operator belongs to a customer tenant, `None` for system /
/// admin actors. Sprint 20 Phase 1 (ADR-0001).
pub fn generate_jwt_with_tenant(
    user_id: &UserId,
    tenant: Option<&TenantId>,
    secret: &[u8],
    expires_in_secs: i64,
) -> Result<String, ApiError> {
    use jsonwebtoken::{encode, EncodingKey, Header};

    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.clone(),
        tenant: tenant.cloned(),
        iat: now,
        exp: now + expires_in_secs,
    };

    let encoding_key = EncodingKey::from_secret(secret);
    encode(&Header::default(), &claims, &encoding_key)
        .map_err(|e| ApiError::InternalError(format!("Failed to generate token: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::users::OperatorRecord;
    use serial_test::serial;

    fn user(id: &str) -> UserId {
        UserId::from_string(id.to_string()).expect("valid UserId")
    }

    /// RAII guard that swaps `$HOME` to a tempdir and restores it on drop.
    /// Mirrors the one in `api::admin::login::tests::RestoreHome` so the
    /// `require_admin` tests can seed a self-contained `users.toml`.
    struct RestoreHome {
        original: Option<std::ffi::OsString>,
    }

    impl RestoreHome {
        fn set(new_home: &std::path::Path) -> Self {
            let original = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", new_home);
            }
            Self { original }
        }
    }

    impl Drop for RestoreHome {
        fn drop(&mut self) {
            unsafe {
                match self.original.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    /// Seed a `users.toml` under a temp `$HOME` with a single operator of
    /// the given `role`. Returns `(tempdir, claims, restore)`.
    fn seed_with_role(user_id: &str, role: &str) -> (tempfile::TempDir, Claims, RestoreHome) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let restore = RestoreHome::set(tmp.path());
        let uid = user(user_id);

        let record = OperatorRecord {
            user_id: uid.clone(),
            display_name: format!("Operator {}", user_id),
            created_at: "2026-04-19T00:00:00Z".to_string(),
            last_login: None,
            role: role.to_string(),
            onboarded_at: None,
            intent_auto_route: false,
        };
        let mut cfg = UsersConfig::default();
        cfg.upsert(record);
        let path = UsersConfig::default_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        cfg.save_to_path(&path).expect("save users.toml");

        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        };
        (tmp, claims, restore)
    }

    #[tokio::test]
    #[serial]
    async fn require_admin_accepts_admin_role() {
        let (_tmp, claims, _restore) = seed_with_role("op-admin", "admin");
        require_admin(&claims).await.expect("admin must pass");
    }

    #[tokio::test]
    #[serial]
    async fn require_admin_rejects_viewer_role() {
        let (_tmp, claims, _restore) = seed_with_role("op-viewer", "viewer");
        let err = require_admin(&claims)
            .await
            .expect_err("viewer must be denied");
        match err {
            ApiError::Forbidden(msg) => {
                assert!(msg.contains("admin role required"), "got: {msg}");
            }
            other => panic!("expected Forbidden, got {:?}", other),
        }
    }

    #[tokio::test]
    #[serial]
    async fn require_admin_rejects_unknown_operator() {
        // users.toml exists, but with a different operator than the claim.
        let (_tmp, _claims, _restore) = seed_with_role("op-registered", "admin");
        let unknown = user("op-ghost");
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: unknown,
            tenant: None,
            iat: now,
            exp: now + 3600,
        };
        let err = require_admin(&claims)
            .await
            .expect_err("unknown operator must be denied");
        match err {
            ApiError::Unauthorized(_) => {}
            other => panic!("expected Unauthorized, got {:?}", other),
        }
    }

    #[test]
    fn test_generate_and_verify_jwt() {
        let secret = b"test-secret-key";
        let user_id = user("test-user-123");

        // 生成 token
        let token = generate_jwt(&user_id, secret, 3600).expect("Failed to generate token");

        // 验证 token
        let claims = verify_jwt(&token, secret).expect("Failed to verify token");

        assert_eq!(claims.sub, user_id);
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_verify_expired_token() {
        let secret = b"test-secret-key";
        let user_id = user("test-user-123");

        // 生成一个已过期的 token (过期1小时)
        let token = generate_jwt(&user_id, secret, -3600).expect("Failed to generate token");

        // 验证应该失败
        let result = verify_jwt(&token, secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_invalid_signature() {
        let secret = b"test-secret-key";
        let wrong_secret = b"wrong-secret-key";
        let user_id = user("test-user-123");

        // 使用正确密钥生成 token
        let token = generate_jwt(&user_id, secret, 3600).expect("Failed to generate token");

        // 使用错误密钥验证应该失败
        let result = verify_jwt(&token, wrong_secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_malformed_token() {
        let secret = b"test-secret-key";
        let malformed_token = "this.is.not.a.valid.jwt";

        let result = verify_jwt(malformed_token, secret);
        assert!(result.is_err());
    }

    #[test]
    fn claims_sub_is_typed_user_id() {
        // Compile-time guarantee: Claims::sub must be UserId. The helper
        // `user_id()` returns a &UserId reference — asserting equality through
        // it ensures the type is the typed id, not a raw String.
        let secret = b"typed-sub-secret";
        let uid = user("typed-user-xyz");
        let token = generate_jwt(&uid, secret, 3600).expect("token");
        let claims = verify_jwt(&token, secret).expect("claims");
        let borrowed: &UserId = claims.user_id();
        assert_eq!(borrowed, &uid);
        assert_eq!(borrowed.as_str(), "typed-user-xyz");
    }

    // ─── Sprint 20 Phase 1 — multi-tenant Claims migration (ADR-0001) ──────

    #[test]
    fn legacy_token_without_tenant_claim_parses_with_none() {
        // Phase 1 backward-compatibility check: tokens issued by
        // `generate_jwt(user, secret, ttl)` (the no-tenant variant)
        // round-trip through verify_jwt with `tenant() == None`.
        // This is the primary correctness invariant for the migration —
        // existing deployments cannot break when Phase 1 lands.
        let secret = b"legacy-token-secret";
        let uid = user("op_legacy");
        let token = generate_jwt(&uid, secret, 3600).expect("token");
        let claims = verify_jwt(&token, secret).expect("claims");
        assert_eq!(claims.user_id(), &uid);
        assert_eq!(
            claims.tenant(),
            None,
            "legacy token must parse with tenant=None (Phase 1 backward-compat)"
        );
    }

    #[test]
    fn token_with_tenant_claim_roundtrips() {
        // Phase 1 capability check: when an issuer calls
        // `generate_jwt_with_tenant`, the resulting token carries the
        // tenant id and verify_jwt surfaces it via Claims::tenant().
        let secret = b"tenant-token-secret";
        let uid = user("op_with_tenant");
        let tenant = TenantId::from_string("acme-corp".to_string()).expect("valid tenant id");

        let token = generate_jwt_with_tenant(&uid, Some(&tenant), secret, 3600).expect("token");
        let claims = verify_jwt(&token, secret).expect("claims");

        assert_eq!(claims.user_id(), &uid);
        assert_eq!(
            claims.tenant(),
            Some(&tenant),
            "tenant must round-trip through encode/decode"
        );
    }

    #[test]
    fn explicit_none_tenant_is_equivalent_to_legacy_path() {
        // generate_jwt_with_tenant(uid, None, ...) must produce the
        // same observable Claims as generate_jwt(uid, ...). This
        // protects callers that programmatically pass through an
        // Option<TenantId> from upstream config.
        let secret = b"explicit-none-secret";
        let uid = user("op_explicit_none");

        let legacy = generate_jwt(&uid, secret, 3600).expect("legacy");
        let explicit = generate_jwt_with_tenant(&uid, None, secret, 3600).expect("explicit");

        let c1 = verify_jwt(&legacy, secret).expect("c1");
        let c2 = verify_jwt(&explicit, secret).expect("c2");

        assert_eq!(c1.user_id(), c2.user_id());
        assert_eq!(c1.tenant(), None);
        assert_eq!(c2.tenant(), None);
    }

    #[test]
    fn generate_jwt_with_typed_user_id() {
        // Ensures generate_jwt accepts &UserId and produces a parseable token.
        let secret = b"generate-typed-secret";
        let uid = UserId::new();
        let token = generate_jwt(&uid, secret, 3600).expect("generate ok");
        // Token must round-trip verify with the same typed id.
        let claims = verify_jwt(&token, secret).expect("verify ok");
        assert_eq!(&claims.sub, &uid);
    }

    #[test]
    fn existing_jwt_tokens_still_parse() {
        // Backwards compatibility: a token whose payload contains `sub` as a
        // plain JSON string (which the prior `sub: String` shape emitted)
        // must decode into the new typed `sub: UserId`.
        use jsonwebtoken::{encode, EncodingKey, Header};

        let secret = b"legacy-compat-secret";
        let now = chrono::Utc::now().timestamp();

        // Construct a legacy-shaped payload manually to simulate a token
        // issued before the typed-sub change.
        #[derive(Serialize)]
        struct LegacyClaims {
            sub: String,
            exp: i64,
            iat: i64,
        }
        let legacy = LegacyClaims {
            sub: "legacy-user-abc".to_string(),
            exp: now + 3600,
            iat: now,
        };
        let token = encode(
            &Header::default(),
            &legacy,
            &EncodingKey::from_secret(secret),
        )
        .expect("encode legacy token");

        let claims = verify_jwt(&token, secret).expect("legacy token must parse");
        assert_eq!(claims.sub.as_str(), "legacy-user-abc");
    }
}
