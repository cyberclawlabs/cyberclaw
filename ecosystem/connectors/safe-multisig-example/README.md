# safe-multisig — Safe (Gnosis Safe) Connector (example)

把 Safe 多签流程接入 CyberClaw 治理链。Safe 的 N-of-M 模型映射到 capability
分级 + `required_approvals`。

## 设计要点

**propose / sign / execute 是三个独立 Capability**：

```
safe.config.read           ── Low      ──┐
safe.tx.list_pending       ── Low      ──│  read-only
safe.tx.simulate           ── Low      ──┘

safe.tx.propose            ── Medium   ──┐  off-chain side effect (Safe Service)
safe.tx.sign               ── High     ──│  off-chain signature
safe.tx.execute            ── Critical ──┘  on-chain side effect

safe.owner.add/remove      ── Critical, 2 approvals
safe.threshold.change      ── Critical, 2 approvals
safe.module.enable         ── Critical, default disabled
```

CyberClaw 不会"代理 owner 把整个流程跑完"。每一次 propose / sign / execute
都是独立的 Capability 调用，分别走治理链 + 审批。Safe owner 多人协作的场景
（"A propose, B sign, C execute"）天然映射到三个 actor 各自走自己的审批
流程。

## 与 wallet-eth 的关系

- `wallet-eth` 是 EOA 钱包（直接持私钥的地址）
- `safe-multisig` 是 Safe 合约钱包（owner 签名 → Safe 转发执行）

二者可以共存：你可以让 Agent 用 wallet-eth 跑日常小额操作，让 safe-multisig
跑金库 / treasury 级操作。

## 配置

```bash
cp safe-multisig-config.toml ~/.cyberclaw/connectors/safe-multisig.toml
cp .env.example .env
```

必需环境变量：

| 变量 | 说明 |
|---|---|
| `SAFE_ADDRESS` | 要操作的 Safe 合约地址 |
| `SAFE_SERVICE_URL` | Safe Transaction Service URL（官方或自建） |
| `ETH_RPC_URL` | 链 RPC 端点 |
| `SAFE_SIGNER_ID` | 本节点扮演的 owner 在 signer-vault 中的 ID |

## Capability 详细规格

### `safe.config.read` (Low)

```json
{}
```

返回：

```json
{
  "address": "0xSafe…",
  "chain_id": 1,
  "version": "1.4.1",
  "owners": ["0x…", "0x…", "0x…"],
  "threshold": 2,
  "nonce": 73,
  "fallback_handler": "0x…",
  "guard": null,
  "modules": []
}
```

### `safe.tx.propose` (Medium, 1 approval)

```json
{
  "to": "0x…",
  "value_wei": "100000000000000000",
  "data": "0x…",
  "operation": 0,
  "safe_tx_gas": 0,
  "base_gas": 0,
  "gas_price": 0,
  "gas_token": "0x0000000000000000000000000000000000000000",
  "refund_receiver": "0x0000000000000000000000000000000000000000"
}
```

行为：
1. PolicyEngine 检查 `input_caps`（max_value_wei / allowed_to_addresses）
2. 触发 1 个审批
3. 通过后向 Safe Transaction Service POST `/transactions`
4. 返回 `safeTxHash`，等待其他 owner 签名

返回：

```json
{
  "safe_tx_hash": "0x…",
  "current_confirmations": 1,
  "threshold": 2,
  "nonce": 73,
  "service_url": "…"
}
```

### `safe.tx.sign` (High, 1 approval)

```json
{ "safe_tx_hash": "0x…" }
```

行为：
1. 拉取 pending tx
2. 构造 EIP-712 typed data
3. 调 signer-vault `signer.sign.typed.eip712`
4. POST signature 到 Safe Service
5. 返回更新后的 confirmations 计数

### `safe.tx.execute` (Critical, 1 approval, must simulate first)

```json
{ "safe_tx_hash": "0x…" }
```

行为：
1. 检查最近是否对该 safeTxHash 跑过 `safe.tx.simulate`，没有就拒
2. 检查签名数是否达到 threshold，没达到就拒
3. 触发审批
4. 通过后构造 `execTransaction()` 调用，由 default_signer 提交（或单独配
   置一个 `relayer_signer`）
5. 提交到链上
6. 返回 tx_hash

返回：

```json
{
  "exec_tx_hash": "0x…",
  "submitted_at": "2026-05-06T…",
  "executor": "0xRelayer…",
  "safe_tx_hash": "0x…"
}
```

## 典型治理流程示例

3-of-5 Safe 给某 vendor 转 50 ETH：

```
[Operator A]  调 safe.tx.propose          (Medium → 1 approval, A self-approves)
              → safeTxHash 0xabc
              → confirmations: 1/3

[Operator B]  调 safe.tx.simulate         (Low, no approval)
              → success, gas_used estimate
              调 safe.tx.sign              (High → 1 approval)
              → confirmations: 2/3

[Operator C]  调 safe.tx.sign              (High → 1 approval)
              → confirmations: 3/3 ✓

[Operator C]  调 safe.tx.execute           (Critical → 1 approval, must simulate first)
              → simulate 已存在 ✓
              → execTransaction() submitted
              → exec_tx_hash captured as Artifact

每一步都有独立审计行；A/B/C 各自的 capability 调用 + 审批 + 签名 + 链上
回执都串在审计链上，可以用 cyberclaw audit verify-chain 离线校验。
```

## 安全提示

- **Safe Service URL** 强烈建议自建。公网官方服务在某些网络可用性不稳。
- **default_signer** 在执行 `execute` 时会消耗本地 gas；建议为 execute 单
  独配置一个低权限 relayer 钱包。
- **module.enable** 默认关闭：installed module 可以绕过 owner 签名。打开
  之前确认 module 合约地址在白名单。
- **threshold.change / owner.add/remove** 默认要 2 个审批，因为这些操作改
  变后续所有 tx 的安全模型。

## 实现状态

manifest + capability 规格在本目录，参考实现在
`crates/cyberclaw-connectors/src/web3/safe/`（v0.2.0 落地）。注册到 registry
后治理链立即生效；capability handler 在 Rust 实现合并前会返回
`Unimplemented`。
