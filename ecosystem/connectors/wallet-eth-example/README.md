# wallet-eth — Ethereum Wallet Connector (example)

EVM 钱包操作通过分级 Capability 接入 CyberClaw 治理链。

## 设计要点

**读 / 模拟 / 签名 / 广播 是四类独立 Capability**，对应风险递增。

```
wallet.eth.balance              ── Low      ──┐
wallet.eth.contract.read        ── Low      ──│  read-only, no side effect
wallet.eth.tx.simulate          ── Low      ──│  dry-run only
wallet.eth.gas.suggest          ── Low      ──┘

wallet.eth.message.sign         ── High     ──┐  needs approval
wallet.eth.typeddata.sign       ── High     ──│
wallet.eth.tx.sign              ── Critical ──┘  signs but does NOT broadcast

wallet.eth.tx.broadcast         ── Critical    side effects start HERE
```

Agent 永远不能"一次过"完成签名 + 广播。`PolicyEngine` 按 capability 粒度
拦截、按风险等级要求审批、按 `required_approvals` 收集审批数、按 `input_caps`
验证调用参数（如最大转账金额、允许收款地址 allowlist）。

## 配置

```bash
cp wallet-eth-config.toml ~/.cyberclaw/connectors/wallet-eth.toml
cp .env.example .env       # 填入 RPC + signer
```

必需环境变量：

| 变量 | 说明 |
|---|---|
| `ETH_RPC_URL` | EVM RPC 端点。建议私有节点或受信 provider |
| `WALLET_SIGNER_ID` | 在 `signer-vault` connector 注册的 signer 名称 |

可选环境变量：

| 变量 | 默认 | 说明 |
|---|---|---|
| `CHAIN_ID` | `1` | EIP-155 chain ID |
| `CHAIN_LABEL` | `mainnet` | 仅用于审计日志的标签 |
| `MAX_TX_VALUE_WEI` | `1e18` (1 ETH) | tx.sign 的转账金额上限 |
| `ALLOWED_TO_LIST` | `(空)` | 允许的收款地址 allowlist (逗号分隔)。空 = 不限 |

## Capability 详细规格

### `wallet.eth.balance` (Low)

```json
{ "address": "0x…", "asset": "ETH" }      // 或 ERC-20: { "address", "asset": "0xtoken…" }
```

返回:

```json
{ "wei": "12345...", "human": "1.234567", "decimals": 18, "block_number": 18000000 }
```

### `wallet.eth.tx.simulate` (Low) — dry-run 必经

```json
{ "to": "0x…", "value_wei": "1000000000000000000", "data": "0x…", "from": "0x…" }
```

返回:

```json
{ "success": true, "gas_used": 21000, "result": "0x…", "revert_reason": null }
```

### `wallet.eth.tx.sign` (Critical) — 治理强审批

输入 EIP-1559 envelope:

```json
{
  "chain_id": 1,
  "to": "0x…",
  "value_wei": "100000000000000000",
  "data": "0x…",
  "max_fee_per_gas": "30000000000",
  "max_priority_fee_per_gas": "1500000000",
  "gas_limit": 21000,
  "nonce": 42
}
```

行为：
1. PolicyEngine 评估，触发 `required_approvals` 个审批
2. 通过后调用 `signer-vault::signer.sign.tx.eip1559`
3. 返回 raw signed tx (`0x…`)，**不广播**
4. 审计行落库，可被 `cyberclaw audit verify-chain` 验证

返回:

```json
{ "raw_signed_tx": "0x02f8…", "tx_hash": "0x…", "from": "0x…" }
```

### `wallet.eth.tx.broadcast` (Critical) — 副作用真正发生

```json
{ "raw_signed_tx": "0x02f8…" }
```

行为：
1. （若 `require_simulation_before_broadcast=true`）先反查最近的同 hash simulate 记录；没有就拒绝
2. 提交到 `ETH_RPC_URL`
3. 返回 mempool 接收回执
4. 审计行 + `Artifact: tx_hash` 立即落库

返回:

```json
{ "tx_hash": "0x…", "submitted_at": "2026-05-06T…", "rpc": "…", "status": "pending" }
```

## 典型治理流程示例

操作员让 Agent 执行 "把 1 ETH 转到 0x…"：

```
Task          : "transfer 1 ETH to 0xRecipient"
  ▼
Resolver      : 选 wallet-eth 作为 connector
  ▼
Execution     : Agent 先调 wallet.eth.tx.simulate          ✅ Low risk → Allow
                  → success, gas_used=21000
                Agent 再调 wallet.eth.tx.sign              🚧 Critical → Ask
                  → Review queue: "Sign 1 ETH transfer to 0xRecipient?"
                Operator approves                          ✓
                  → signer-vault signs                     ✓
                  → returns raw_signed_tx                  ✓
                Agent 最后调 wallet.eth.tx.broadcast       🚧 Critical → Ask
                  → Review queue: "Broadcast pre-signed tx 0xabc…?"
                Operator approves                          ✓
                  → submitted to mempool                   ✓
                  → tx_hash captured as Artifact
                  → audit row chained
```

每一步独立审批；任何一步拒绝都不影响前序产出（已签名但未广播的 tx 可以
作弃 / 重试）。

## 网络风险提示

- `ETH_RPC_URL` 不要用公开 free-tier，否则 RPC 限流会让模拟与广播变得不可
  预测。私有节点或付费 provider 是基线。
- `finality = safe` 是写路径默认。读路径如果对状态鲜度敏感可改 `latest`。
- 多链场景：每条链建议起一个独立 connector instance（不要把 chain_id 设为
  用户参数）。

## 实现状态

manifest + capability 规格在本目录，参考实现在
`crates/cyberclaw-connectors/src/web3/wallet_eth/`（v0.2.0 落地）。
当前可以用此清单注册到 registry，capability 调用会显示 `Unimplemented` 直到
Rust 实现合并；治理链 / 审批 / 审计在 manifest 注册的那一刻就生效。
