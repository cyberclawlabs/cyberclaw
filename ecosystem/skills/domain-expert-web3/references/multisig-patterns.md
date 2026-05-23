# Safe Multisig Patterns

## Topologies

### 1. Single-Safe ops
- All treasury in one Safe.
- Simpler audit, single point of failure for the threshold.
- OK for < $1M.

### 2. Two-tier (hot / cold)
- Hot Safe: 3/5, fast ops, daily spending limit.
- Cold Safe: 5/9, reserves, no automated tx, used quarterly.
- Cold → Hot via timelocked module (48h).
- Standard for DAOs > $10M.

### 3. Multi-sig federation
- Region/team Safes (3/5 each), each is a signer on the federation Safe.
- Used by orgs with global ops — EU/US/APAC each operate their region.
- Risk: signatures are recursive; a regional compromise = one vote at federation level.

## Threshold math

For `M of N`, attacker compromise sets = `C(N,M)`. Concrete:

| M/N | C(N,M) |
|---|---|
| 2/3 | 3 |
| 3/5 | 10 |
| 4/7 | 35 |
| 5/9 | 126 |
| 7/11 | 330 |

**Rule**: log2(C(N,M)) ≥ 7 (i.e. ≥ 128 sets) for "treasury grade".

## Key-handling discipline

- Each signer uses an independent HW wallet (Ledger, Trezor, GridPlus, Keystone).
- No two signers share firmware version + brand simultaneously.
- At least one air-gapped signer (Keystone / Ledger Nano X in airplane mode + PSBT-like flow).
- Mnemonic backups stored separately (Cryptosteel + safe-deposit box, not photo).
- Annual signer rotation: replace ≥ 1 signer per year, retire old key by removing from Safe (not by "trust them").

## Safe modules — what they are and when to use them

A Safe module can execute transactions on the Safe **without threshold**. Powerful but dangerous.

- **Allowance Module** (OZ): per-token, per-delegate periodic spending limit. Use for daily ops budget.
- **Recovery Module** (Safe{RecoveryHub}): time-delayed owner replacement if N owners are offline.
- **Roles Module** (Zodiac): role-based scoped exec; grants a role permission to call only specific function selectors.
- **Reality Module** (Zodiac + Reality.eth): off-chain proposal (Snapshot) → on-chain execution after challenge window.

**Risk**: enabling a module is irrevocable until a disable-module SafeTx executes. Audit each module's source code before enabling. Set up Forta alert for `EnabledModule`/`DisabledModule` events.

## Operational recovery — owner lost

Pre-incident:
- Maintain owner registry: name, contact, key fingerprint, backup HW location.
- Quarterly "tabletop": owners practice signing a no-op SafeTx.

Incident:
- Lost a single owner in M of N where remaining ≥ M: business as usual; rotate the lost key out via `swapOwner` SafeTx.
- Lost > (N-M) owners: Safe is bricked. Recovery requires either:
  - A pre-installed recovery module timing out and replacing owners.
  - Or, for protocols, social recovery via governance + proxy admin (only if Safe is a Gnosis-Safe-as-proxy-admin pattern).
- No recovery path exists for a vanilla M-of-N Safe with no module if you lose too many keys.

## Audit trail

- Every SafeTx has an executed `Transaction` event with `txHash`, `payment`, `success` flag.
- Subscribe in your operational dashboard:
  - Etherscan API → tx history
  - Safe Transaction Service API → tx queue + signatures pre-execution
  - Forta bot → real-time alerts
- Reconcile monthly: expected ops txs vs actual on-chain.

## Common operational anti-patterns

- All N owners on the same Slack/Telegram team (social engineering blast radius).
- Same email-recovery on all HW wallet vendor accounts (one phish drops all signers).
- Storing owner addresses in a public Notion page (target list for spear-phish).
- Owners using their daily-driver wallet for personal DeFi from the same machine that holds Safe HW keys (clipboard hijack risk).
- "Just this once" lowering threshold for an emergency tx without a counter-proposal to raise it back.
