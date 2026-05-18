use cyberclaw_core::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub trait CaseManager: Send + Sync {
    fn upsert_case(&self, case: Case) -> anyhow::Result<Case>;
    fn get_case(&self, id: &CaseId) -> anyhow::Result<Option<Case>>;
    fn list_cases(&self) -> anyhow::Result<Vec<Case>>;
}

/// InMemory implementation of CaseManager for development and testing
#[derive(Debug, Clone, Default)]
pub struct InMemoryCaseManager {
    cases: Arc<RwLock<BTreeMap<CaseId, Case>>>,
}

impl InMemoryCaseManager {
    pub fn new() -> Self {
        Self {
            cases: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl CaseManager for InMemoryCaseManager {
    fn upsert_case(&self, case: Case) -> anyhow::Result<Case> {
        let mut cases = self.cases.blocking_write();
        cases.insert(case.id.clone(), case.clone());
        Ok(case)
    }

    fn get_case(&self, id: &CaseId) -> anyhow::Result<Option<Case>> {
        let cases = self.cases.blocking_read();
        Ok(cases.get(id).cloned())
    }

    fn list_cases(&self) -> anyhow::Result<Vec<Case>> {
        let cases = self.cases.blocking_read();
        Ok(cases.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str, actor_type: ActorType) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[test]
    fn test_upsert_and_get_case() {
        let manager = InMemoryCaseManager::new();
        let case = Case {
            id: CaseId::new(),
            title: "Test Case".to_string(),
            summary: "Test summary".to_string(),
            kind: CaseKind::Incident,
            status: CaseStatus::Open,
            owner_tenant: TenantId::from_string("test-tenant".to_string()).unwrap(),
            created_by: create_test_actor("test-user", "Test User", ActorType::Human),
            created_at: Utc::now(),
            labels: vec!["security".to_string()],
            metadata: BTreeMap::new(),
        };

        let case_id = case.id.clone();
        manager.upsert_case(case.clone()).unwrap();

        let retrieved = manager.get_case(&case_id).unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Test Case");
    }

    #[test]
    fn test_upsert_updates_existing_case() {
        let manager = InMemoryCaseManager::new();
        let case_id = CaseId::new();

        let case1 = Case {
            id: case_id.clone(),
            title: "Original Title".to_string(),
            summary: "Original summary".to_string(),
            kind: CaseKind::Incident,
            status: CaseStatus::Open,
            owner_tenant: TenantId::from_string("test-tenant".to_string()).unwrap(),
            created_by: create_test_actor("test-user", "Test User", ActorType::Human),
            created_at: Utc::now(),
            labels: vec![],
            metadata: BTreeMap::new(),
        };

        manager.upsert_case(case1).unwrap();

        let case2 = Case {
            id: case_id.clone(),
            title: "Updated Title".to_string(),
            summary: "Updated summary".to_string(),
            kind: CaseKind::Incident,
            status: CaseStatus::InProgress,
            owner_tenant: TenantId::from_string("test-tenant".to_string()).unwrap(),
            created_by: create_test_actor("test-user", "Test User", ActorType::Human),
            created_at: Utc::now(),
            labels: vec![],
            metadata: BTreeMap::new(),
        };

        manager.upsert_case(case2).unwrap();

        let retrieved = manager.get_case(&case_id).unwrap().unwrap();
        assert_eq!(retrieved.title, "Updated Title");
        assert_eq!(retrieved.status, CaseStatus::InProgress);
    }

    #[test]
    fn test_list_cases() {
        let manager = InMemoryCaseManager::new();

        for i in 0..3 {
            let case = Case {
                id: CaseId::new(),
                title: format!("Case {}", i),
                summary: "Test summary".to_string(),
                kind: CaseKind::Incident,
                status: CaseStatus::Open,
                owner_tenant: TenantId::from_string("test-tenant".to_string()).unwrap(),
                created_by: create_test_actor("test-user", "Test User", ActorType::Human),
                created_at: Utc::now(),
                labels: vec![],
                metadata: BTreeMap::new(),
            };
            manager.upsert_case(case).unwrap();
        }

        let cases = manager.list_cases().unwrap();
        assert_eq!(cases.len(), 3);
    }
}
