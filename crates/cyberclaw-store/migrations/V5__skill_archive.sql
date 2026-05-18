-- Skill evolutionary archive persistence (V5)
-- Created: 2026-04-18
--
-- Stores SkillVariant records so evolution history survives across sessions.
-- Corresponds to MigrationRunner version 5 in crates/cyberclaw-store/src/migration.rs.
-- Applied automatically by the in-code MigrationRunner; this file exists for
-- documentation parity with V1__initial_schema.sql.

CREATE TABLE IF NOT EXISTS skill_variants (
    variant_id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    parent_variant_id TEXT,
    score REAL NOT NULL,
    child_count INTEGER NOT NULL DEFAULT 0,
    track TEXT NOT NULL,
    patch_artifact_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_variants_skill_id ON skill_variants(skill_id);
CREATE INDEX IF NOT EXISTS idx_skill_variants_parent ON skill_variants(parent_variant_id);
