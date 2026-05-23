//! SkillBinder — keyword-driven auto-binding of domain expert skills.
//!
//! ## Why
//!
//! `req.skill_ids` lets a caller name skills explicitly. Some skills, however,
//! should bind automatically when the user's prompt is *in their domain*
//! (Web3 multisig, SOC incident response, DevOps release engineering, …) so
//! the LLM gets the right vocabulary even when the caller didn't think to
//! ask for it.
//!
//! ## Semantics
//!
//! Each candidate skill's `manifest.yaml` may declare:
//!
//! ```yaml
//! spec:
//!   auto_bind:
//!     keywords:
//!       - any: [multisig, "Safe", Gnosis]
//!       - any: [USDC, ETH, gas]
//!     priority: 80
//! ```
//!
//! Matching is **AND-of-OR**:
//!
//! - Each list element under `keywords` is an "any-group" (OR within the group).
//! - The skill matches the prompt only when **every** any-group has at least
//!   one keyword present (case-insensitive substring match on the prompt).
//!
//! Multiple matched skills are returned ordered by `priority` (high → low).
//!
//! ## Boundary
//!
//! - SkillBinder is a *pure* keyword matcher; it does not load SKILL.md
//!   bodies and does not mutate state. Callers (e.g. chat_handler) read the
//!   matched skill names, locate the SKILL.md, and inject the body into the
//!   system prompt.
//! - SkillBinder is **additive** to `req.skill_ids`: explicit bindings always
//!   win; auto-bound skills are appended (deduplicated).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One auto-bind rule loaded from a single `manifest.yaml`.
#[derive(Debug, Clone)]
pub struct AutoBindRule {
    /// Skill name (matches the directory name under `installed/`).
    pub skill_name: String,
    /// Path to the skill directory (so callers can locate SKILL.md).
    pub skill_dir: PathBuf,
    /// AND-of-OR groups. Each inner Vec is an OR group; all groups must match
    /// for the rule to fire.
    pub keyword_groups: Vec<Vec<String>>,
    /// Higher = preferred when multiple skills match.
    pub priority: i32,
}

impl AutoBindRule {
    /// Test the rule against a prompt. Case-insensitive substring match. All
    /// groups must have at least one hit. Returns `true` only if the rule has
    /// at least one non-empty group (a rule with no groups never matches —
    /// avoids accidental "always-bind").
    pub fn matches(&self, prompt: &str) -> bool {
        let prompt_lc = prompt.to_lowercase();
        self.matches_lower(&prompt_lc)
    }

    /// Same as [`matches`], but assumes the prompt is already lowercased.
    /// Use this on the hot path when matching one prompt against many rules.
    pub fn matches_lower(&self, prompt_lc: &str) -> bool {
        if self.keyword_groups.is_empty() {
            return false;
        }
        for group in &self.keyword_groups {
            if group.is_empty() {
                // An empty OR-group cannot be satisfied; rule fails closed.
                return false;
            }
            let any_hit = group.iter().any(|kw| {
                let kw_lc = kw.to_lowercase();
                !kw_lc.is_empty() && prompt_lc.contains(&kw_lc)
            });
            if !any_hit {
                return false;
            }
        }
        true
    }
}

/// Top-level manifest deserialization. We tolerate every other field — only
/// `spec.auto_bind` matters here.
#[derive(Debug, Deserialize)]
struct ManifestEnvelope {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    spec: Option<ManifestSpec>,
}

#[derive(Debug, Deserialize)]
struct ManifestSpec {
    #[serde(default)]
    auto_bind: Option<AutoBindSpec>,
}

#[derive(Debug, Deserialize)]
struct AutoBindSpec {
    #[serde(default)]
    keywords: Vec<KeywordGroup>,
    #[serde(default)]
    priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct KeywordGroup {
    #[serde(default)]
    any: Vec<String>,
}

/// Parse a single `manifest.yaml` body. Returns `Ok(None)` when the manifest
/// has no `spec.auto_bind` — that's not an error, just a non-binder skill.
///
/// `skill_dir` is the directory containing the manifest; `skill_name` falls
/// back to the directory's `file_name` if the manifest omits `name`.
pub fn parse_manifest_auto_bind(
    yaml: &str,
    skill_dir: &Path,
) -> Result<Option<AutoBindRule>, serde_yaml::Error> {
    let env: ManifestEnvelope = serde_yaml::from_str(yaml)?;
    let spec = match env.spec {
        Some(s) => s,
        None => return Ok(None),
    };
    let ab = match spec.auto_bind {
        Some(a) => a,
        None => return Ok(None),
    };
    let groups: Vec<Vec<String>> = ab
        .keywords
        .into_iter()
        .map(|g| g.any.into_iter().filter(|k| !k.trim().is_empty()).collect())
        .filter(|g: &Vec<String>| !g.is_empty())
        .collect();
    if groups.is_empty() {
        return Ok(None);
    }
    let skill_name = env
        .name
        .or_else(|| {
            skill_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| skill_dir.display().to_string());
    Ok(Some(AutoBindRule {
        skill_name,
        skill_dir: skill_dir.to_path_buf(),
        keyword_groups: groups,
        priority: ab.priority.unwrap_or(0),
    }))
}

/// In-memory registry of auto-bind rules.
#[derive(Debug, Default, Clone)]
pub struct SkillBinder {
    rules: Vec<AutoBindRule>,
}

impl SkillBinder {
    /// Build an empty binder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a rule. Method intentionally named `push` to avoid the
    /// `clippy::should_implement_trait` lint that fires on `add` (which
    /// shadows `std::ops::Add::add`).
    pub fn push(&mut self, rule: AutoBindRule) {
        self.rules.push(rule);
    }

    /// Number of registered rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the binder has zero rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Scan an `installed/` directory: each immediate child that contains a
    /// `manifest.yaml` is parsed; if it carries a usable `auto_bind` block,
    /// the rule is registered.
    ///
    /// Unreadable files / broken YAML are logged at `warn` and skipped — a
    /// single broken skill must not break the whole registry.
    pub fn load_from_dir(&mut self, installed_dir: &Path) -> std::io::Result<usize> {
        let mut added = 0usize;
        if !installed_dir.is_dir() {
            return Ok(0);
        }
        for entry in std::fs::read_dir(installed_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(?err, "SkillBinder: read_dir entry failed");
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest = path.join("manifest.yaml");
            if !manifest.exists() {
                continue;
            }
            let body = match std::fs::read_to_string(&manifest) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(path = %manifest.display(), ?err, "SkillBinder: read failed");
                    continue;
                }
            };
            match parse_manifest_auto_bind(&body, &path) {
                Ok(Some(rule)) => {
                    tracing::debug!(
                        skill = %rule.skill_name,
                        groups = rule.keyword_groups.len(),
                        priority = rule.priority,
                        "SkillBinder: registered auto-bind rule"
                    );
                    self.rules.push(rule);
                    added += 1;
                }
                Ok(None) => {
                    // Manifest has no auto_bind block — not an error.
                }
                Err(err) => {
                    tracing::warn!(
                        path = %manifest.display(),
                        ?err,
                        "SkillBinder: invalid manifest yaml, skipping"
                    );
                }
            }
        }
        Ok(added)
    }

    /// Match every registered rule against the prompt. Returns matched rules
    /// in priority-descending order. Stable on equal priority (preserves
    /// registration order).
    pub fn match_prompt(&self, prompt: &str) -> Vec<&AutoBindRule> {
        let lc = prompt.to_lowercase();
        let mut hits: Vec<&AutoBindRule> =
            self.rules.iter().filter(|r| r.matches_lower(&lc)).collect();
        // Stable sort, descending priority (Reverse to flip the ordering).
        hits.sort_by_key(|r| std::cmp::Reverse(r.priority));
        hits
    }

    /// Borrow the underlying rules (for diagnostics / tests).
    pub fn rules(&self) -> &[AutoBindRule] {
        &self.rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn rule(name: &str, groups: Vec<Vec<&str>>, priority: i32) -> AutoBindRule {
        AutoBindRule {
            skill_name: name.into(),
            skill_dir: PathBuf::from("/tmp/fake"),
            keyword_groups: groups
                .into_iter()
                .map(|g| g.into_iter().map(|s| s.into()).collect())
                .collect(),
            priority,
        }
    }

    #[test]
    fn empty_rule_never_matches() {
        let r = rule("empty", vec![], 0);
        assert!(!r.matches("anything"));
    }

    #[test]
    fn single_group_or_matches() {
        let r = rule("web3", vec![vec!["multisig", "safe"]], 0);
        assert!(r.matches("how do i sign a multisig tx"));
        assert!(r.matches("Safe transaction"));
        assert!(!r.matches("hello world"));
    }

    #[test]
    fn and_of_or_requires_all_groups() {
        let r = rule(
            "web3",
            vec![vec!["multisig", "safe"], vec!["usdc", "eth"]],
            0,
        );
        assert!(r.matches("move usdc through a Safe multisig"));
        // Only first group hits.
        assert!(!r.matches("how does Safe work in general"));
        // Only second group hits.
        assert!(!r.matches("eth price went up"));
    }

    #[test]
    fn case_insensitive() {
        let r = rule("soc", vec![vec!["SIEM"]], 0);
        assert!(r.matches("siem alert came in"));
        assert!(r.matches("SIEM Alert"));
        assert!(r.matches("our Siem was overwhelmed"));
    }

    #[test]
    fn empty_group_in_rule_fails_closed() {
        let r = rule("bad", vec![vec!["x"], vec![]], 0);
        assert!(!r.matches("x"));
    }

    #[test]
    fn priority_orders_matches() {
        let mut b = SkillBinder::new();
        b.push(rule("low", vec![vec!["alert"]], 10));
        b.push(rule("high", vec![vec!["alert"]], 80));
        b.push(rule("mid", vec![vec!["alert"]], 50));
        let hits = b.match_prompt("an alert fired");
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].skill_name, "high");
        assert_eq!(hits[1].skill_name, "mid");
        assert_eq!(hits[2].skill_name, "low");
    }

    #[test]
    fn parse_manifest_with_auto_bind() {
        let yaml = r#"
apiVersion: cyberclaw.io/v2
kind: Skill
name: my-skill
spec:
  format: claude-compatible
  auto_bind:
    keywords:
      - any: [foo, bar]
      - any: [baz]
    priority: 42
"#;
        let dir = TempDir::new().unwrap();
        let r = parse_manifest_auto_bind(yaml, dir.path())
            .unwrap()
            .expect("rule expected");
        assert_eq!(r.skill_name, "my-skill");
        assert_eq!(r.keyword_groups.len(), 2);
        assert_eq!(r.priority, 42);
        assert!(r.matches("a foo with baz"));
        assert!(!r.matches("just foo"));
    }

    #[test]
    fn parse_manifest_without_auto_bind_returns_none() {
        let yaml = r#"
apiVersion: cyberclaw.io/v2
kind: Skill
name: plain-skill
spec:
  format: claude-compatible
"#;
        let dir = TempDir::new().unwrap();
        let r = parse_manifest_auto_bind(yaml, dir.path()).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn parse_manifest_falls_back_to_dir_name() {
        let yaml = r#"
apiVersion: cyberclaw.io/v2
spec:
  auto_bind:
    keywords:
      - any: [x]
    priority: 1
"#;
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("my-cool-skill");
        fs::create_dir_all(&sub).unwrap();
        let r = parse_manifest_auto_bind(yaml, &sub).unwrap().unwrap();
        assert_eq!(r.skill_name, "my-cool-skill");
    }

    #[test]
    fn load_from_dir_picks_up_manifests() {
        let dir = TempDir::new().unwrap();
        let installed = dir.path();
        let sk_a = installed.join("alpha");
        fs::create_dir_all(&sk_a).unwrap();
        fs::write(
            sk_a.join("manifest.yaml"),
            r#"
apiVersion: cyberclaw.io/v2
name: alpha
spec:
  auto_bind:
    keywords:
      - any: [foo]
    priority: 5
"#,
        )
        .unwrap();
        // Skill without auto_bind — should be skipped.
        let sk_b = installed.join("beta");
        fs::create_dir_all(&sk_b).unwrap();
        fs::write(
            sk_b.join("manifest.yaml"),
            "name: beta\nspec: { format: claude-compatible }\n",
        )
        .unwrap();
        // Garbage YAML — should be tolerated (warn + skip), not panic.
        let sk_c = installed.join("gamma");
        fs::create_dir_all(&sk_c).unwrap();
        fs::write(sk_c.join("manifest.yaml"), ":\n\t bad yaml\n  - x\n").unwrap();

        let mut binder = SkillBinder::new();
        let added = binder.load_from_dir(installed).unwrap();
        assert_eq!(added, 1, "only alpha contributes a rule");
        assert_eq!(binder.rules().len(), 1);
        assert_eq!(binder.rules()[0].skill_name, "alpha");
    }

    #[test]
    fn load_from_missing_dir_is_ok() {
        let mut binder = SkillBinder::new();
        let n = binder
            .load_from_dir(Path::new("/no/such/directory"))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn match_prompt_dedupes_via_priority_sort() {
        let mut b = SkillBinder::new();
        b.push(rule("web3", vec![vec!["multisig"], vec!["usdc"]], 80));
        b.push(rule("devops", vec![vec!["deploy"]], 70));
        let hits = b.match_prompt(
            "we are deploying a multisig contract upgrade, USDC treasury at risk",
        );
        let names: Vec<&str> = hits.iter().map(|r| r.skill_name.as_str()).collect();
        assert_eq!(names, vec!["web3", "devops"]);
    }

    // ---- Test 2: web3 and SOC auto-bind keyword triggers ----

    #[test]
    fn test_auto_bind_web3_keyword_triggers_skill() {
        let mut binder = SkillBinder::new();
        // Register a rule that fires on web3 keywords (multisig OR safe.execTransaction)
        binder.push(rule(
            "domain-expert-web3",
            vec![vec!["multisig", "safe.execTransaction", "gnosis"]],
            80,
        ));

        // "multisig" should trigger
        let hits = binder.match_prompt("how do I call safe.execTransaction on a multisig wallet?");
        assert!(
            !hits.is_empty(),
            "expected domain-expert-web3 to match web3 prompt"
        );
        assert_eq!(hits[0].skill_name, "domain-expert-web3");

        // Prompt with "gnosis" alone should also match
        let hits2 = binder.match_prompt("explain the Gnosis Safe architecture");
        assert!(
            !hits2.is_empty(),
            "expected domain-expert-web3 to match gnosis prompt"
        );

        // Unrelated prompt must not match
        let hits3 = binder.match_prompt("what is the weather today?");
        assert!(
            hits3.is_empty(),
            "non-web3 prompt must not trigger domain-expert-web3"
        );
    }

    #[test]
    fn test_auto_bind_soc_keyword_triggers_skill() {
        let mut binder = SkillBinder::new();
        // Register a rule that fires on SOC/incident-response keywords
        binder.push(rule(
            "domain-expert-soc",
            vec![vec!["MITRE", "incident response", "ATT&CK", "playbook"]],
            75,
        ));

        // "MITRE" should trigger
        let hits = binder.match_prompt("map this alert to the MITRE ATT&CK framework");
        assert!(
            !hits.is_empty(),
            "expected domain-expert-soc to match MITRE prompt"
        );
        assert_eq!(hits[0].skill_name, "domain-expert-soc");

        // "incident response" should trigger
        let hits2 = binder.match_prompt("walk me through an incident response playbook");
        assert!(
            !hits2.is_empty(),
            "expected domain-expert-soc to match incident response prompt"
        );

        // Unrelated prompt must not match
        let hits3 = binder.match_prompt("deploy the new web3 contract");
        assert!(
            hits3.is_empty(),
            "non-SOC prompt must not trigger domain-expert-soc"
        );
    }
}
