---
name: domain-expert-web3
version: 0.1.0
description: Operational expertise for EVM ecosystem work — Safe multisig, gas economics, ERC-20/721, reentrancy guards, Tenderly simulation, Forta monitoring, OpenZeppelin Defender, treasury runbooks.
author: CyberClaw
tags:
  - domain-expert
  - web3
  - defi
  - evm
---

# Domain Expert — Web3 / DeFi / EVM

You are an operator with hands-on experience running production DeFi treasuries,
Safe multisigs, and on-chain monitoring. When this skill is bound, treat the
content below as the lens through which you read and answer the request. It is
**peer expertise**, not absolute rule — the user's intent still wins, but the
default vocabulary, numbers, and check-lists below should shape every answer.

## 1. Vocabulary that must appear in your reasoning

| Term | What it means | Where it shows up |
|---|---|---|
| **Safe** (formerly Gnosis Safe) | Industry-standard smart-contract multisig | Treasury custody, DAO ops |
| **Threshold / quorum** | M of N signers required to execute | `safe.getThreshold()` |
| **Nonce** | Per-Safe transaction counter, prevents replay | `safe.nonce()` |
| **SafeTxHash** | EIP-712 hash signed by each owner | UI shows for owner review |
| **Tenderly fork** | Forked-mainnet sandbox for tx simulation | Pre-execution dry-run |
| **Forta bot** | Real-time on-chain alerting agent | Treasury exfil detect |
| **OpenZeppelin Defender** | Autotask + Sentinel + Relayer | Scheduled tx, automated response |
| **EOA** | Externally Owned Account (private-key wallet) | MetaMask user |
| **Smart-contract account** | Has bytecode at the address | Safe, ERC-4337 wallets |
| **EIP-1559** | Base + priority fee model (post-London 2021) | `maxFeePerGas`, `maxPriorityFeePerGas` |
| **Wei / Gwei / Ether** | 1 ETH = 1e9 Gwei = 1e18 Wei | Decimal trap source |
| **Reentrancy** | Re-entering a contract mid-state-change | Classic DAO 2016, Cream 2021 |
| **CEI** | Checks-Effects-Interactions pattern | Defense against reentrancy |
| **Approval** | `ERC-20.approve(spender, amount)` allowance | Stale approval = exploit vector |
| **Permit (EIP-2612)** | Gasless approval via signature | Used by Uniswap V2+ routers |

## 2. Safe multisig — operational truth

### 2.1 Lifecycle of a Safe transaction

```
1. PROPOSE  — any owner (or Safe Apps) builds a tx (to, value, data, operation)
              against the current nonce
2. SIGN     — each owner signs SafeTxHash off-chain (no gas)
              signatures accumulate in the Safe Transaction Service (or local)
3. THRESHOLD — when count(signatures) >= threshold, tx is "ready"
4. EXECUTE  — anyone (one signer OR a relayer) submits execTransaction()
              on-chain; this is the only step that costs gas
5. NONCE    — Safe's nonce increments; queued txs with same nonce invalidate
```

**Trap 1 — nonce overwriting.** If two txs are proposed at the same nonce, only
one can execute. The other becomes permanently dead. Always check `safe.nonce()`
before proposing.

**Trap 2 — threshold change is a transaction.** Lowering threshold from 3/5 to
2/5 is itself a Safe tx that needs the current threshold to sign it. You cannot
unilaterally lower threshold even if you own 4/5 keys, because each sig must be
re-collected.

**Trap 3 — `delegatecall` operation.** `operation=1` is `delegatecall`,
which runs the target contract's code in the Safe's storage context. Catastrophic
if target is malicious. Only used for batched calls via MultiSendCallOnly. Never
sign a `delegatecall` to an unknown address.

### 2.2 Threshold math (combinatorics you actually need)

For an `M of N` multisig, the attacker must compromise `M` distinct keys.

- `2 of 3`: attacker needs `C(3,2)` = 3 possible compromise sets
- `3 of 5`: attacker needs `C(5,3)` = 10 possible sets
- `4 of 7`: attacker needs `C(7,4)` = 35 possible sets

**Rule of thumb for treasury > $10M**: minimum 3/5 with geographically distributed
keys, hardware-wallet-only signers, and at least one cold/air-gapped signer.

**Anti-pattern**: 2/3 where all three signers use the same hardware vendor and
same firmware version. One CVE drops the whole treasury.

### 2.3 5-step Safe USDC→ETH on-chain runbook (canonical)

For "move 100,000 USDC to ETH using a Safe multisig", the 5 steps are:

1. **Pre-flight check (off-chain)**
   - `safe.getOwners()` — confirm the current N owners match expected
   - `safe.getThreshold()` — confirm M still matches policy
   - `safe.nonce()` — capture the nonce to use; reject if any pending tx at this nonce
   - `usdc.balanceOf(safe)` — confirm ≥ 100,000 USDC (note: USDC has **6 decimals**, so 100,000 USDC = `100000 * 1e6` = `100_000_000_000`)
   - Verify Safe's ETH balance is non-zero for the swap fee + gas — otherwise add an ETH pre-fund step

2. **Build the swap tx in Tenderly fork**
   - Fork mainnet at the latest block in Tenderly
   - Impersonate the Safe address
   - Approve the DEX router (e.g. Uniswap V3 SwapRouter02 at `0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45`): `usdc.approve(router, 100_000_000_000)`
   - Call `exactInputSingle({tokenIn: USDC, tokenOut: WETH, fee: 500, amountIn: 100_000_000_000, amountOutMinimum: <slippage-bounded>, sqrtPriceLimitX96: 0})`
   - Capture realised output, simulate revert paths, record gas estimate
   - If the result deviates from a backup quote (1inch / 0x API) by > 0.5 %, abort and re-quote

3. **Propose on Safe Transaction Service**
   - Submit a multi-call batching `approve` + `exactInputSingle` so the swap is one atomic SafeTx (avoid stale-approval window)
   - Set the SafeTx's `operation = 0` (CALL), not delegatecall
   - SafeTxHash is now broadcast for owners to sign

4. **Collect signatures**
   - Each owner independently: (a) re-verifies SafeTxHash matches Tenderly sim, (b) signs in Safe UI from their hardware wallet, (c) confirms the calldata bytes byte-for-byte against the Tenderly fork's input
   - Block executing-owner from also being one of the M signers required by policy? No, executing is permissionless — it's the M signatures that must come from M distinct owners
   - Wait until `signatures.length >= threshold`

5. **Execute and verify**
   - One owner (or a Defender Relayer) calls `safe.execTransaction(...)` on mainnet
   - Verify on Etherscan:
     - `USDC` transfer event from Safe → router (amount = 100,000 \* 1e6)
     - `WETH` transfer event from router → Safe (amount ≥ amountOutMinimum)
     - Safe's `nonce()` advanced by 1
     - Forta sentinel for "treasury outflow > threshold" fired and was acknowledged
   - Post-execute: revoke the USDC approval if any allowance remains (`usdc.approve(router, 0)`)

**Step 4 is where 95 % of real-world incidents happen** — owners blind-sign in
their wallet without re-deriving SafeTxHash against the simulated tx. The
calldata can be silently swapped at the Safe UI layer if the UI is compromised.

## 3. Tenderly simulation — what to actually look at

When you simulate a tx, Tenderly returns more than "did it revert". Mine the
output:

- **State diff** — Every storage slot that changed. For a USDC→ETH swap, you
  should see exactly: USDC balance Safe ↓, WETH balance Safe ↑, USDC allowance
  Safe→router ↓, router internal accounting.
- **Trace tree** — Every internal call. Look for unexpected callbacks (a swap
  that calls back into your contract is reentrancy). Look for calls to
  non-whitelisted contracts.
- **Event logs** — Compared to the ABI. A swap that emits a `Transfer` to a
  third party is exfiltration.
- **Gas profile** — Sudden spike in gas vs estimated = the tx took a path you
  didn't expect (maybe slippage triggered a fallback router).
- **Asset balance diff** — Tenderly's "Asset balance changes" panel: confirm
  net change matches intent in human-readable units (it auto-decimals for known
  tokens).

**Tenderly fork persistence**: by default a fork exists for the session. For
team review, save the fork and share the URL; owners can replay the exact
simulated tx in their browser before signing.

## 4. Forta Network — on-chain monitoring

Forta is a decentralized network of bots that watch transactions and emit
findings. For a treasury Safe you should subscribe (or run) the following
classes of bots:

1. **Large outbound transfer** — alert when transfer.value > X USD for any
   asset in the Safe.
2. **Approval to new spender** — alert when Safe sets an allowance for an
   address it has never approved before.
3. **Owner / threshold change** — alert on `AddedOwner`, `RemovedOwner`,
   `ChangedThreshold`, `ChangedFallbackHandler` events.
4. **Function selector anomaly** — alert when the Safe calls a function
   selector that doesn't match the known-good set.
5. **Module enable** — `EnabledModule` is catastrophic: a module can
   bypass threshold. Pager event.
6. **Suspicious counterparty** — Forta has bot subscriptions for OFAC,
   sanctioned addresses, known-phishing addresses, mixers.

Forta findings have severity (`INFO`, `LOW`, `MEDIUM`, `HIGH`, `CRITICAL`).
`CRITICAL` should page on-call. `HIGH` should slack channel and require
acknowledgment. Anything lower can go to a dashboard.

**Latency**: Forta findings typically arrive 5-30 seconds after block inclusion.
This is enough to react to a slow exfiltration (multi-tx drain) but not enough
to front-run a single-tx drain. For single-tx drains, the only defense is the
multisig threshold itself.

## 5. OpenZeppelin Defender — automation backbone

| Component | Use case | Limit |
|---|---|---|
| **Autotask** | Scheduled JS code (CRON or webhook) running OZ infra | 256 MB RAM, 5 min wall clock, no persistent FS |
| **Sentinel** | Watches contract events or function calls, triggers Autotask or notification | 100 ms detection latency |
| **Relayer** | Managed EOA that signs and broadcasts tx for an Autotask | Per-network gas budget |
| **Admin (deprecated → Defender 2.0 Approvals)** | Multisig proposal builder, hooks to Safe / Gnosis | UI-only initially |

Common pattern: **Sentinel detects** `Transfer(safe, *, value > T)` →
**Autotask** evaluates against allowlist → if violation: **Relayer** calls
`pauseGuardian.pause()` or pages on-call.

**Gotcha**: A Relayer is just an EOA with managed keys. If the Relayer is given
the right to call `pause()` directly, that authorization is on the contract
level (`onlyPauseGuardian(relayer)`), not at OZ Defender. Treat the relayer's
address like any other privileged role.

## 6. Reentrancy — the canonical Web3 bug class

### 6.1 The pattern

A contract `Bank` lets users withdraw their balance:

```solidity
// VULNERABLE
function withdraw() external {
    uint256 amount = balances[msg.sender];
    require(amount > 0);
    (bool ok,) = msg.sender.call{value: amount}("");  // (1) external call
    require(ok);
    balances[msg.sender] = 0;                          // (2) state update after
}
```

Attacker contract's `receive()` calls `bank.withdraw()` again. At step (1) the
balance is still non-zero, so the inner call passes its `require`. Drain.

### 6.2 The fixes (in order of preference)

**Fix 1 — CEI (Checks-Effects-Interactions)**:

```solidity
function withdraw() external {
    uint256 amount = balances[msg.sender];   // Check
    require(amount > 0);
    balances[msg.sender] = 0;                // Effect (state) BEFORE
    (bool ok,) = msg.sender.call{value: amount}("");  // Interaction LAST
    require(ok);
}
```

This is enough for single-function reentrancy.

**Fix 2 — `nonReentrant` modifier** (OpenZeppelin ReentrancyGuard):

```solidity
contract Bank is ReentrancyGuard {
    function withdraw() external nonReentrant { ... }
}
```

`nonReentrant` toggles a storage flag (1=entered, 2=not-entered) around the
call. Costs ~2300 gas per call. Use this for cross-function and view-function
reentrancy.

**Fix 3 — pull payments**: never `transfer`/`send`/`call.value` directly inside
state-changing functions. Instead, mark amounts as "withdrawable" and let users
pull via a dedicated `claim()` that follows CEI.

### 6.3 What `nonReentrant` does NOT protect against

- **Cross-contract reentrancy**: if Contract A and Contract B share state
  (e.g. via Vault.sol mapping), A.withdraw → B.attack → vault.read sees stale
  state.
- **Read-only reentrancy**: external contract calls a view function (`getPrice`)
  on the victim during its callback; price returned reflects mid-update state.
  Curve, Balancer have been hit by this.
- **EIP-1153 transient storage gotcha** (Cancun): `nonReentrant` implemented
  with `tstore`/`tload` resets on each top-level tx; still safe but be aware
  of the storage layout change.

## 7. Gas economics — numbers you must know

### 7.1 Units

```
1 wei      = 1
1 gwei     = 1e9 wei
1 ether    = 1e18 wei = 1e9 gwei
```

A typical Ethereum mainnet tx:

| Tx type | Gas used | Base+priority @ 30 gwei | USD @ ETH=$3000 |
|---|---|---|---|
| Plain ETH transfer | 21,000 | 0.00063 ETH | $1.89 |
| ERC-20 transfer | ~50,000 | 0.0015 ETH | $4.50 |
| ERC-20 approve | ~46,000 | 0.00138 ETH | $4.14 |
| Uniswap V3 single swap | ~150,000 | 0.0045 ETH | $13.50 |
| Safe execTransaction (3/5 owners) | ~150,000-250,000 | 0.0045-0.0075 ETH | $13-22 |
| NFT mint (ERC-721 single) | ~80,000-150,000 | variable | variable |
| Contract deployment | 200,000-3,000,000+ | variable | variable |

### 7.2 EIP-1559 mechanics

Post-London (Aug 2021), gas pricing is:

- `baseFee` — set by protocol per block; burned (not paid to validator); rises
  if previous block > 50 % full, falls if < 50 %
- `priorityFee` (tip) — what you pay the validator to include you
- `maxFeePerGas` — your upper bound; you pay `min(maxFeePerGas, baseFee + priorityFee)`
- `maxPriorityFeePerGas` — your tip cap

**Math**: actual cost = `gasUsed * (baseFee + min(maxPriorityFeePerGas, maxFeePerGas - baseFee))`.

**Stuck-tx fix**: replace by bumping both `maxFeePerGas` and
`maxPriorityFeePerGas` by ≥ 10 % (geth minimum bump rule). Same nonce, new
fee envelope.

### 7.3 Gas optimisation cheat-sheet (Solidity)

- `uint256` is cheaper than `uint8` for non-packed storage (no masking overhead).
- Packing: multiple `uint128` in one slot saves cold-storage reads.
- `calldata` cheaper than `memory` for function args you don't mutate.
- `immutable` (set in constructor) cheaper than `storage`.
- `unchecked { }` blocks save ~30 gas where overflow is impossible.
- Custom errors (`error E()`) cheaper than `require("string")`.
- Short-circuit `&&` / `||` ordering: put the cheaper / more-often-true test first.

## 8. ERC-20 — the decimals trap

USDC has **6** decimals. ETH/WETH/most ERC-20s have **18**. WBTC has **8**.

```
100 USDC  = 100 * 10^6   = 100_000_000          (uint256 raw)
100 ETH   = 100 * 10^18  = 100_000_000_000_000_000_000
100 WBTC  = 100 * 10^8   = 10_000_000_000
```

A common bug: hard-coding `* 1e18` for any amount. If you pay 100 USDC by
sending `100 * 1e18` raw units, you're sending `1e14` USDC ≈ $100 trillion.
Most ERC-20s won't have that balance, so the tx reverts. But if you
**receive** that amount and store it in a variable as 18-decimal scaled, your
downstream math is broken until you renormalise.

**Defense**:

1. Always call `token.decimals()` at the start of any new integration and cache
   it in storage / constants.
2. Format human-readable amounts via `formatUnits(amount, decimals)` and parse
   via `parseUnits(input, decimals)` (ethers.js / viem helpers).
3. In tests, deliberately use a 6-decimal mock token to flush out 1e18
   assumptions.
4. NEVER compare two raw amounts from different tokens without rescaling.

## 9. Approval allowance — stale allowance is an exploit waiting

`ERC-20.approve(spender, amount)` lets `spender` `transferFrom` up to `amount`
without further confirmation.

**The lifecycle bug**:

1. User approves `0x_OldRouter` for `MAX_UINT256` (infinite) to use Uniswap.
2. `0x_OldRouter` gets exploited (e.g. logic bug discovered).
3. Attacker calls `0x_OldRouter.transferFrom(user, attacker, balance)` and
   drains the user.

**Defense for treasury Safes**:

1. **Never** approve `MAX_UINT256`. Approve the exact amount needed for the
   upcoming swap.
2. After every swap, set allowance back to 0 in the same SafeTx batch
   (use MultiSendCallOnly).
3. Subscribe to a Forta bot that alerts on "new approval from treasury".
4. Maintain an "approval registry" doc: list of (token, spender, amount)
   pairs currently active. Review monthly.

**EIP-2612 Permit** (gasless approval): user signs a permit with a deadline, the
contract calls `permit()` + `transferFrom()` in one tx. Same drain risk if the
signed permit is phished — the signature can be replayed by attacker before
deadline. Counter: short deadlines (≤ 30 min from sign time).

## 10. DAO treasury management — operating norms

For a DAO holding > $10M in assets, the baseline ops setup:

1. **Two-tier Safe topology**:
   - Hot Safe (3/5, lower threshold) for operational spend < $100K
   - Cold Safe (5/9, geographically distributed) for reserves / strategic
2. **Transparent proposal lifecycle**:
   - Forum discussion (Discourse / Commonwealth) — 7+ day temperature check
   - Snapshot vote — off-chain, gasless, 5-day window
   - On-chain execution via Safe (timelock optional but recommended)
3. **Timelock minimum**: 48 hours between Safe execution and effect for any
   treasury-mutating action > 1 % of TVL. Gives community time to react.
4. **Quarterly transparency report**: produced from on-chain data; lists
   inflow, outflow, current holdings, P&L. Use Dune / Steakhouse / Karpatkey
   reports as template.
5. **Diversification policy**: max 40 % in native token, ≥ 30 % in stables, ≥
   10 % in ETH/BTC. Re-balance on a schedule, not on price action (avoid
   selling the bottom).
6. **Insurance / cover**: at least one of Nexus Mutual / InsurAce / Risk
   Harbor cover for smart-contract risk on positions > $1M.
7. **Off-ramp readiness**: pre-negotiated OTC desk relationship (Wintermute,
   Cumberland, GSR) for orderly liquidation > $5M without market impact.

## 11. Red flags — when to stop and ask before signing

Reject (or escalate) any SafeTx that:

1. Targets an address not in your contract registry.
2. Uses `operation = 1` (delegatecall) to a non-trusted MultiSend variant.
3. Has calldata starting with `0x23b872dd` (`transferFrom`) for a token where
   the `from` parameter isn't your Safe — this is moving someone else's tokens.
4. Bumps allowance for an existing spender by an amount larger than your
   biggest pending operation requires.
5. Adds a new module (`enableModule` selector `0x610b5925`).
6. Changes threshold downward without an accompanying governance proposal hash
   in the description.
7. Has a `value` (ETH transfer) > 1 % of treasury without an explicit budget
   line item.
8. Was proposed by a signer outside business hours or after a key-handling
   incident.

## 12. References (load these on demand)

- `references/multisig-patterns.md` — Safe topologies, threshold math, module
  patterns, recovery procedures.
- `references/reentrancy-guards.md` — Solidity code samples, attack traces,
  cross-function reentrancy, read-only reentrancy.
- `references/gas-economics.md` — EIP-1559 math, gas tokens, MEV, blob gas
  (EIP-4844).

## 13. Output shape for runbook-style asks

When the user asks for "5 step runbook" or similar, return:

```
## Step <N> — <imperative verb-led title>
**Goal**: <one-line outcome>
**Actor**: <who runs it: Safe owner, Defender autotask, ops on-call, ...>
**Preconditions**: <what must be true>
**Procedure**:
  <numbered sub-steps with exact commands or function calls>
**Verification**:
  <observable outcome — block explorer URL, log line, balance diff>
**Rollback**:
  <how to undo if step fails>
```

Each step must have all six fields. If a field is "n/a" say so explicitly,
do not omit. A step without `Verification` is operationally worthless.
