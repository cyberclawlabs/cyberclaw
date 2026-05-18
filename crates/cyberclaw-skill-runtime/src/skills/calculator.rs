//! CalculatorSkill: performs basic arithmetic operations.
//!
//! # Input Schema
//! ```json
//! {
//!   "op":  "add" | "sub" | "mul" | "div",
//!   "a":   <number>,
//!   "b":   <number>
//! }
//! ```
//!
//! # Output Schema
//! ```json
//! { "result": <number> }
//! ```

use async_trait::async_trait;

use crate::handler::SkillHandler;

/// A skill that performs basic arithmetic: add, sub, mul, div.
pub struct CalculatorSkill;

#[async_trait]
impl SkillHandler for CalculatorSkill {
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let op = input["op"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'op' field (expected string)"))?;

        let a = input["a"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'a' field (expected number)"))?;

        let b = input["b"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'b' field (expected number)"))?;

        let result = match op {
            "add" => a + b,
            "sub" => a - b,
            "mul" => a * b,
            "div" => {
                // Safe float comparison: check if b is close to zero
                if b.abs() < f64::EPSILON {
                    anyhow::bail!("division by zero");
                }
                a / b
            }
            other => anyhow::bail!(
                "unknown operation '{}'; expected add, sub, mul, or div",
                other
            ),
        };

        Ok(serde_json::json!({ "result": result }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn calc(op: &str, a: f64, b: f64) -> anyhow::Result<f64> {
        let skill = CalculatorSkill;
        let input = serde_json::json!({ "op": op, "a": a, "b": b });
        let output = skill.handle(input).await?;
        output["result"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("result is not a number"))
    }

    #[tokio::test]
    async fn test_add() {
        assert_eq!(calc("add", 3.0, 4.0).await.unwrap(), 7.0);
    }

    #[tokio::test]
    async fn test_sub() {
        assert_eq!(calc("sub", 10.0, 3.0).await.unwrap(), 7.0);
    }

    #[tokio::test]
    async fn test_mul() {
        assert_eq!(calc("mul", 3.0, 4.0).await.unwrap(), 12.0);
    }

    #[tokio::test]
    async fn test_div() {
        assert_eq!(calc("div", 12.0, 4.0).await.unwrap(), 3.0);
    }

    #[tokio::test]
    async fn test_div_by_zero_returns_error() {
        let result = calc("div", 1.0, 0.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("division by zero"));
    }

    #[tokio::test]
    async fn test_div_by_negative_zero_returns_error() {
        let result = calc("div", 1.0, -0.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("division by zero"));
    }

    #[tokio::test]
    async fn test_div_by_subnormal_returns_error() {
        // f64::EPSILON / 2.0 is smaller than EPSILON, so it should be treated as zero
        let result = calc("div", 1.0, f64::EPSILON / 2.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("division by zero"));
    }

    #[tokio::test]
    async fn test_unknown_op_returns_error() {
        let result = calc("mod", 10.0, 3.0).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown operation"));
    }

    #[tokio::test]
    async fn test_missing_op_returns_error() {
        let skill = CalculatorSkill;
        let input = serde_json::json!({ "a": 1.0, "b": 2.0 });
        let result = skill.handle(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("'op'"));
    }

    #[tokio::test]
    async fn test_missing_operand_returns_error() {
        let skill = CalculatorSkill;
        let input = serde_json::json!({ "op": "add", "a": 1.0 });
        let result = skill.handle(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("'b'"));
    }

    #[tokio::test]
    async fn test_negative_numbers() {
        assert_eq!(calc("add", -5.0, 3.0).await.unwrap(), -2.0);
        assert_eq!(calc("mul", -2.0, -3.0).await.unwrap(), 6.0);
    }
}
