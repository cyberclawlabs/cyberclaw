//! Clarify Card 模块 - Agent→User 结构化澄清请求
//!
//! 对齐 Claude Code 官方 `AskUserQuestionTool` schema：
//! - 每次请求 1-4 个问题
//! - 每个问题 2-4 个选项
//! - option.description 必填
//! - "Other" 选项由平台自动提供，不允许 Agent 手动添加

use crate::ids::{AgentId, ClarifyId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// 一个澄清选项，对应 Claude 官方 `questionOptionSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarifyOption {
    /// 显示给用户的简短标签（1-5 词）
    pub label: String,
    /// 解释该选项含义、权衡或后果（必填，非空）
    pub description: String,
    /// 聚焦时展示的预览内容（代码片段、diff 等），可选
    pub preview: Option<String>,
}

/// 一个澄清问题，对应 Claude 官方 `questionSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarifyQuestion {
    /// 完整问题文本，应以问号结尾
    pub question: String,
    /// 可供选择的选项，必须 2-4 个
    pub options: Vec<ClarifyOption>,
    /// 是否允许多选
    #[serde(default)]
    pub multi_select: bool,
}

/// 澄清请求的状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClarifyStatus {
    /// 等待用户回答
    Pending,
    /// 用户已回答
    Resolved,
    /// 超时未回答
    TimedOut,
}

/// 完整的澄清请求，由 Agent 构造并提交给 ClarifyService。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarifyRequest {
    pub id: ClarifyId,
    /// 所属对话 ID（使用 String，ConversationId 不在 core 范围内）
    pub conversation_id: String,
    /// 发起请求的 Agent
    pub agent_id: AgentId,
    /// 问题列表，必须 1-4 个
    pub questions: Vec<ClarifyQuestion>,
    /// 来源标识（用于分析追踪，如 "remember"）
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: ClarifyStatus,
    /// 用户回答映射：questionText → 选中的 label
    pub answers: Option<BTreeMap<String, String>>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// 用户对一个 ClarifyRequest 的完整回答集合
///
/// 对齐 Claude Code AskUserQuestionTool outputSchema:
/// `answers: z.record(z.string(), z.string())`
/// key = questionText, value = answer string
/// multi-select 答案逗号分隔
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarifyAnswer {
    /// questionText → answer string；multi-select 答案逗号分隔
    pub answers: BTreeMap<String, String>,
}

impl ClarifyAnswer {
    /// 创建一个空答案集合
    pub fn new() -> Self {
        Self {
            answers: BTreeMap::new(),
        }
    }

    /// 插入一个问题的答案
    pub fn insert(&mut self, question: impl Into<String>, answer: impl Into<String>) {
        self.answers.insert(question.into(), answer.into());
    }
}

impl Default for ClarifyAnswer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Clarify 操作的错误类型
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClarifyError {
    #[error("invalid clarify form: {reason}")]
    InvalidForm { reason: String },
    #[error("clarify request already resolved")]
    AlreadyResolved,
    #[error("clarify request already timed out")]
    AlreadyTimedOut,
    #[error("clarify request not found")]
    NotFound,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl ClarifyRequest {
    /// 验证 ClarifyRequest 的结构约束，对齐 Claude 官方 schema。
    ///
    /// # Errors
    ///
    /// 任何约束违反均返回 `ClarifyError::InvalidForm { reason }` 携带明确原因。
    pub fn validate(&self) -> Result<(), ClarifyError> {
        // 1. questions 数量约束 [1, 4]
        if self.questions.is_empty() {
            return Err(ClarifyError::InvalidForm {
                reason: "questions must not be empty (min 1)".to_string(),
            });
        }
        if self.questions.len() > 4 {
            return Err(ClarifyError::InvalidForm {
                reason: format!("too many questions: {} (max 4)", self.questions.len()),
            });
        }

        // 2. question 文本唯一性（大小写敏感，与 Claude 官方一致）
        {
            let mut seen = std::collections::HashSet::new();
            for q in &self.questions {
                if !seen.insert(q.question.as_str()) {
                    return Err(ClarifyError::InvalidForm {
                        reason: format!("duplicate question text: {:?}", q.question),
                    });
                }
            }
        }

        // 3. 逐问题验证
        for q in &self.questions {
            // question 文本非空
            if q.question.trim().is_empty() {
                return Err(ClarifyError::InvalidForm {
                    reason: "question text must not be empty".to_string(),
                });
            }

            // options 数量约束 [2, 4]
            if q.options.len() < 2 {
                return Err(ClarifyError::InvalidForm {
                    reason: format!(
                        "question {:?} must have at least 2 options (got {})",
                        q.question,
                        q.options.len()
                    ),
                });
            }
            if q.options.len() > 4 {
                return Err(ClarifyError::InvalidForm {
                    reason: format!(
                        "question {:?} must have at most 4 options (got {})",
                        q.question,
                        q.options.len()
                    ),
                });
            }

            // option label 唯一性（同一 question 内）
            {
                let mut seen_labels = std::collections::HashSet::new();
                for opt in &q.options {
                    if !seen_labels.insert(opt.label.as_str()) {
                        return Err(ClarifyError::InvalidForm {
                            reason: format!(
                                "duplicate option label {:?} in question {:?}",
                                opt.label, q.question
                            ),
                        });
                    }
                }
            }

            for opt in &q.options {
                // label 不能是 "Other"（大小写不敏感）
                if opt.label.trim().to_lowercase() == "other" {
                    return Err(ClarifyError::InvalidForm {
                        reason: format!(
                            "option label 'Other' is reserved and provided automatically by the platform (found {:?} in question {:?})",
                            opt.label, q.question
                        ),
                    });
                }

                // description 非空
                if opt.description.trim().is_empty() {
                    return Err(ClarifyError::InvalidForm {
                        reason: format!(
                            "option {:?} in question {:?} must have a non-empty description",
                            opt.label, q.question
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ClarifyQueue trait
// ---------------------------------------------------------------------------

/// Clarify 队列的抽象接口
///
/// 定义在 core 层，避免 agent-runtime ↔ control-plane 循环依赖。
/// 负责 ClarifyRequest 的生命周期管理：enqueue → resolve / mark_timeout
#[async_trait]
pub trait ClarifyQueue: Send + Sync + std::fmt::Debug {
    /// 入队一个新的 ClarifyRequest
    ///
    /// 先调用 `req.validate()`，再检查 id 唯一性，最后插入。
    async fn enqueue(&self, req: ClarifyRequest) -> Result<(), ClarifyError>;

    /// 以给定 answer 将请求标记为 Resolved（幂等）
    ///
    /// - 若 status == Resolved，返回原 request（不报错）
    /// - 若 status == TimedOut，返回 `AlreadyTimedOut`
    /// - 若 id 不存在，返回 `NotFound`
    async fn resolve(
        &self,
        id: &ClarifyId,
        answer: ClarifyAnswer,
    ) -> Result<ClarifyRequest, ClarifyError>;

    /// 将请求标记为 TimedOut（幂等）
    ///
    /// - 若 status == TimedOut，返回当前 request
    /// - 若 status == Resolved，返回 `AlreadyResolved`
    /// - 若 id 不存在，返回 `NotFound`
    async fn mark_timeout(&self, id: &ClarifyId) -> Result<ClarifyRequest, ClarifyError>;

    /// 按 id 查找请求，不存在返回 None
    async fn get(&self, id: &ClarifyId) -> Option<ClarifyRequest>;

    /// 列出所有 status == Pending 的请求
    async fn list_pending(&self) -> Vec<ClarifyRequest>;

    /// 列出 status == Resolved 且 resolved_at >= since 的请求，按 resolved_at 升序
    async fn list_resolved(&self, since: DateTime<Utc>) -> Vec<ClarifyRequest>;

    /// 列出所有 status == TimedOut 的请求，按 created_at 降序，最多返回 limit 条。
    ///
    /// 提供默认实现（返回空列表），使外部实现无需立即跟进。
    /// `InMemoryClarifyQueue` 覆盖此方法以执行真实扫描。
    async fn list_timed_out(&self, limit: usize) -> Result<Vec<ClarifyRequest>, ClarifyError> {
        let _ = limit;
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_option(label: &str, description: &str) -> ClarifyOption {
        ClarifyOption {
            label: label.to_string(),
            description: description.to_string(),
            preview: None,
        }
    }

    fn make_question(text: &str, options: Vec<ClarifyOption>) -> ClarifyQuestion {
        ClarifyQuestion {
            question: text.to_string(),
            options,
            multi_select: false,
        }
    }

    fn make_request(questions: Vec<ClarifyQuestion>) -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: "conv-test-001".to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions,
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    // --- validate: happy paths ---

    #[test]
    fn test_validate_accepts_minimal_valid_request() {
        let req = make_request(vec![make_question(
            "Which staging environment?",
            vec![
                make_option("staging-a", "Use staging A environment"),
                make_option("staging-b", "Use staging B environment"),
            ],
        )]);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_accepts_max_questions_and_options() {
        let questions: Vec<ClarifyQuestion> = (0..4)
            .map(|i| {
                make_question(
                    &format!("Question {}?", i),
                    (0..4)
                        .map(|j| {
                            make_option(
                                &format!("option-{}-{}", i, j),
                                &format!("Description for option {}-{}", i, j),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let req = make_request(questions);
        assert!(req.validate().is_ok());
    }

    // --- validate: questions count ---

    #[test]
    fn test_validate_rejects_zero_questions() {
        let req = make_request(vec![]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("empty"), "reason: {reason}");
        }
    }

    #[test]
    fn test_validate_rejects_five_questions() {
        let questions: Vec<ClarifyQuestion> = (0..5)
            .map(|i| {
                make_question(
                    &format!("Question {}?", i),
                    vec![
                        make_option("a", "Option A description"),
                        make_option("b", "Option B description"),
                    ],
                )
            })
            .collect();
        let req = make_request(questions);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("5"), "reason: {reason}");
        }
    }

    // --- validate: options count ---

    #[test]
    fn test_validate_rejects_one_option() {
        let req = make_request(vec![make_question(
            "Which staging environment?",
            vec![make_option("staging-a", "Use staging A environment")],
        )]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("at least 2"), "reason: {reason}");
        }
    }

    #[test]
    fn test_validate_rejects_five_options() {
        let options: Vec<ClarifyOption> = (0..5)
            .map(|i| make_option(&format!("opt-{}", i), "Some description"))
            .collect();
        let req = make_request(vec![make_question("Which staging environment?", options)]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("at most 4"), "reason: {reason}");
        }
    }

    // --- validate: uniqueness ---

    #[test]
    fn test_validate_rejects_duplicate_question_text() {
        let req = make_request(vec![
            make_question(
                "Which staging environment?",
                vec![
                    make_option("staging-a", "Use staging A"),
                    make_option("staging-b", "Use staging B"),
                ],
            ),
            make_question(
                "Which staging environment?",
                vec![
                    make_option("prod-a", "Use prod A"),
                    make_option("prod-b", "Use prod B"),
                ],
            ),
        ]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("duplicate question"), "reason: {reason}");
        }
    }

    #[test]
    fn test_validate_rejects_duplicate_option_label_within_question() {
        let req = make_request(vec![make_question(
            "Which staging environment?",
            vec![
                make_option("staging-a", "Use staging A"),
                make_option("staging-a", "Also staging A but different text"),
            ],
        )]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(
                reason.contains("duplicate option label"),
                "reason: {reason}"
            );
        }
    }

    #[test]
    fn test_validate_allows_same_option_label_across_questions() {
        // 不同 question 之间允许相同 label
        let req = make_request(vec![
            make_question(
                "First question?",
                vec![
                    make_option("yes", "Confirm yes"),
                    make_option("no", "Confirm no"),
                ],
            ),
            make_question(
                "Second question?",
                vec![
                    make_option("yes", "Agree to proceed"),
                    make_option("no", "Do not proceed"),
                ],
            ),
        ]);
        assert!(req.validate().is_ok());
    }

    // --- validate: "Other" label rejection ---

    #[test]
    fn test_validate_rejects_option_label_other() {
        for label in &["Other", "other", " OTHER ", "Other "] {
            let req = make_request(vec![make_question(
                "Which staging environment?",
                vec![
                    make_option("staging-a", "Use staging A"),
                    make_option(label, "Some description"),
                ],
            )]);
            let err = req.validate().unwrap_err();
            assert!(
                matches!(err, ClarifyError::InvalidForm { .. }),
                "label {:?} should be rejected",
                label
            );
            if let ClarifyError::InvalidForm { reason } = err {
                assert!(
                    reason.contains("reserved"),
                    "label {:?}: reason: {reason}",
                    label
                );
            }
        }
    }

    // --- validate: empty text ---

    #[test]
    fn test_validate_rejects_empty_description() {
        let req = make_request(vec![make_question(
            "Which staging environment?",
            vec![
                make_option("staging-a", "Use staging A"),
                make_option("staging-b", "   "), // only whitespace
            ],
        )]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("non-empty description"), "reason: {reason}");
        }
    }

    #[test]
    fn test_validate_rejects_empty_question_text() {
        let req = make_request(vec![make_question(
            "   ", // only whitespace
            vec![
                make_option("staging-a", "Use staging A"),
                make_option("staging-b", "Use staging B"),
            ],
        )]);
        let err = req.validate().unwrap_err();
        assert!(matches!(err, ClarifyError::InvalidForm { .. }));
        if let ClarifyError::InvalidForm { reason } = err {
            assert!(reason.contains("empty"), "reason: {reason}");
        }
    }

    // --- serde roundtrips ---

    #[test]
    fn test_clarify_request_roundtrip_json() {
        let req = make_request(vec![make_question(
            "Which staging environment?",
            vec![
                make_option("staging-a", "Use staging A environment"),
                make_option("staging-b", "Use staging B environment"),
            ],
        )]);
        let json = serde_json::to_string(&req).unwrap();
        let decoded: ClarifyRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.conversation_id, decoded.conversation_id);
        assert_eq!(req.questions.len(), decoded.questions.len());
        assert_eq!(req.questions[0].question, decoded.questions[0].question);
        assert_eq!(
            req.questions[0].options[0].label,
            decoded.questions[0].options[0].label
        );
    }

    #[test]
    fn test_clarify_option_roundtrip_json() {
        let opt = ClarifyOption {
            label: "staging-a".to_string(),
            description: "Use staging A environment".to_string(),
            preview: Some("# Staging A\nURL: https://staging-a.example.com".to_string()),
        };
        let json = serde_json::to_string(&opt).unwrap();
        let decoded: ClarifyOption = serde_json::from_str(&json).unwrap();
        assert_eq!(opt, decoded);
    }

    #[test]
    fn test_clarify_status_roundtrip_json() {
        for status in [
            ClarifyStatus::Pending,
            ClarifyStatus::Resolved,
            ClarifyStatus::TimedOut,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let decoded: ClarifyStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, decoded);
        }
    }

    #[test]
    fn test_clarify_answer_roundtrip_json() {
        let mut answer = ClarifyAnswer::new();
        answer.insert("Which staging environment?", "staging-a");
        answer.insert("Which region?", "us-east-1");

        let json = serde_json::to_string(&answer).unwrap();
        let decoded: ClarifyAnswer = serde_json::from_str(&json).unwrap();
        assert_eq!(answer, decoded);
        assert_eq!(
            decoded.answers.get("Which staging environment?").unwrap(),
            "staging-a"
        );
        assert_eq!(decoded.answers.get("Which region?").unwrap(), "us-east-1");
    }

    #[test]
    fn test_clarify_answer_default_is_empty() {
        let answer = ClarifyAnswer::default();
        assert!(answer.answers.is_empty());
    }
}
