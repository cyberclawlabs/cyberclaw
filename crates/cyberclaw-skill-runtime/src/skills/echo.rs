//! EchoSkill: returns the input unchanged.

use async_trait::async_trait;

use crate::handler::SkillHandler;

/// A skill that echoes its input back as the output.
///
/// Useful for testing skill wiring without implementing any logic.
pub struct EchoSkill;

#[async_trait]
impl SkillHandler for EchoSkill {
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_echo_returns_input() {
        let skill = EchoSkill;
        let input = serde_json::json!({ "key": "value", "num": 42 });
        let output = skill.handle(input.clone()).await.unwrap();
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn test_echo_null_input() {
        let skill = EchoSkill;
        let output = skill.handle(serde_json::Value::Null).await.unwrap();
        assert_eq!(output, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn test_echo_array_input() {
        let skill = EchoSkill;
        let input = serde_json::json!([1, 2, 3]);
        let output = skill.handle(input.clone()).await.unwrap();
        assert_eq!(output, input);
    }
}
