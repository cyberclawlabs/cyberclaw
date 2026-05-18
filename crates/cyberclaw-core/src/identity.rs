use crate::ids::{ActorId, NodeId, TenantId};
use serde::{Deserialize, Serialize};

/// Represents the identity of a caller making a request to the platform.
///
/// Used for authorization checks at dispatch boundaries to ensure only
/// authenticated identities can submit tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Identity {
    /// Anonymous/unauthenticated caller. Task dispatch is always rejected.
    Anonymous,
    /// System/internal caller. Always authorized.
    System,
    /// Human or service user with a specific ID and assigned roles.
    User { id: String, roles: Vec<String> },
    /// Service account with explicit permissions.
    Service {
        name: String,
        permissions: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorType {
    Human,
    Agent,
    System,
    Connector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorRef {
    pub id: ActorId,
    pub actor_type: ActorType,
    pub tenant_id: Option<TenantId>,
    pub home_node_id: Option<NodeId>,
    pub display_name: String,
}

impl Identity {
    /// Convert Identity to ActorRef for audit purposes.
    ///
    /// This conversion enables proper audit trail by transforming authentication
    /// context (Identity) into a structured audit subject (ActorRef).
    ///
    /// # Parameters
    /// - `tenant_id`: Optional tenant context for multi-tenancy support
    ///
    /// # Returns
    /// - `Some(ActorRef)` for all authenticated identities (System, User, Service)
    /// - `Some(ActorRef)` for Anonymous with a special "anonymous" ID for audit completeness
    ///
    /// # Security
    /// Even anonymous requests are tracked with a special actor ID to maintain
    /// audit chain integrity as required by OWASP A09 (Logging and Monitoring).
    pub fn to_actor_ref(&self, tenant_id: Option<TenantId>) -> Option<ActorRef> {
        match self {
            Identity::Anonymous => {
                // SECURITY: Even anonymous requests should be audited for security monitoring
                // Using a well-known "anonymous" ID instead of None to maintain audit chain
                let id = ActorId::from_string("anonymous".to_string())
                    .expect("'anonymous' is a valid ActorId");
                Some(ActorRef {
                    id,
                    actor_type: ActorType::System,
                    tenant_id,
                    home_node_id: None,
                    display_name: "Anonymous".to_string(),
                })
            }
            Identity::System => {
                let id = ActorId::from_string("system".to_string())
                    .expect("'system' is a valid ActorId");
                Some(ActorRef {
                    id,
                    actor_type: ActorType::System,
                    tenant_id,
                    home_node_id: None,
                    display_name: "System".to_string(),
                })
            }
            Identity::User { id, roles: _ } => {
                // Attempt to convert user ID to ActorId
                // If conversion fails (invalid ID format), we return None
                let actor_id = ActorId::from_string(id.clone()).ok()?;
                Some(ActorRef {
                    id: actor_id,
                    actor_type: ActorType::Human,
                    tenant_id,
                    home_node_id: None,
                    display_name: id.clone(),
                })
            }
            Identity::Service {
                name,
                permissions: _,
            } => {
                // Service accounts are treated as Agents for audit purposes
                let actor_id = ActorId::from_string(name.clone()).ok()?;
                Some(ActorRef {
                    id: actor_id,
                    actor_type: ActorType::Agent,
                    tenant_id,
                    home_node_id: None,
                    display_name: name.clone(),
                })
            }
        }
    }
}
