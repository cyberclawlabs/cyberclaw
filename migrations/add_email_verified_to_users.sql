-- Migration: add_email_verified_to_users
-- Adds email_verified column to users, backfills from verifications, adds index

BEGIN;

-- Step 1: Add the column (not null default already enforced by DEFAULT false)
ALTER TABLE users ADD COLUMN email_verified BOOLEAN NOT NULL DEFAULT false;

-- Step 2: Backfill existing rows from verifications (1 row per user)
-- Only update rows where verifications exists; users without a row stay false (the default)
UPDATE users
SET    email_verified = true
FROM   verifications v
WHERE  users.id = v.user_id
AND    v.verified_at IS NOT NULL   -- only count as verified if verification was completed
AND    v.expires_at IS NULL        -- or: v.expires_at > now() if you track expiry
AND    users.email_verified = false;  -- skip already-true rows to avoid unnecessary writes

-- Step 3: Add index on email_verified for filter queries (e.g. "show unverified users")
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_users_email_verified ON users (email_verified)
WHERE email_verified = false;   -- partial index: only unverified rows need heavy indexing

COMMIT;
