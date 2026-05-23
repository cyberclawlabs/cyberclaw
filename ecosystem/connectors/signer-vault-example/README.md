# signer-vault — Signer Vault Connector (example)

把"持有私钥"这一职责从所有业务 connector 中抽出。`wallet-eth` /
`safe-multisig` / 任何未来的链 connector 都不接触私钥；它们只持有
`signer_id`，签名调用走这里。

## 设计要点

1. **业务 connector 不接触私钥**。它们引用 `signer_id` 调 vault 的
   `signer.sign.*` capability。
2. **后端可插拔**。同一 vault 可以同时挂载 keystore JSON / AWS KMS / GCP
   KMS / YubiHSM / Ledger / Trezor / 远程签名器。
3. **签名按 envelope 分类**。`personal_sign` / `EIP-712` / EIP-1559 tx 是
   三个独立 capability。`signer.sign.raw.bytes`（任意字节）默认关闭——大多
   数业务想签的都是某种 typed envelope，"raw bytes" 通常意味着调用方在绕
   过类型检查。

## 后端支持

| backend | 适用场景 | 私钥所在 |
|---|---|---|
| `keystore` | 本地开发 / 自托管 EOA | encrypted JSON V3 keystore + 密码（密码走 env）|
| `aws-kms` | 生产 / 多操作员 | AWS KMS asymmetric ECDSA_SECG_P256K1 key |
| `gcp-kms` | 生产 / 多操作员 | GCP KMS EC_SIGN_SECP256K1_SHA256 key |
| `yubihsm` | 高安全 / 单机 | YubiHSM2 设备 |
| `ledger` / `trezor` | 冷备 / 关键操作 | 物理硬件钱包 |
| `remote` | SSV / Web3Signer / 自建签名器 | 远端 RPC，TLS mTLS |

每个 signer 注册时声明自己的 backend 类型；业务 connector 拿到的只是
`signer_id`。

## Capability 分级

```
signer.list                ── Low      ──┐  read-only
signer.export.pubkey       ── Low      ──┘

signer.sign.message        ── High     ──┐  EIP-191 personal_sign
signer.sign.typed.eip712   ── High     ──│  EIP-712 typed data
signer.sign.tx.eip1559     ── High     ──┘  EIP-1559 tx envelope

signer.sign.raw.bytes      ── Critical    任意字节签名（默认关闭）
```

## 配置

```bash
cp signer-vault-config.toml ~/.cyberclaw/connectors/signer-vault.toml
cp .env.example .env
```

每个 signer 的环境变量按 backend 类型不同：

**keystore backend:**

```
KEYSTORE_PATH=/path/to/keystore.json
KEYSTORE_PASSWORD=…                 # 走 env，不要写到 toml
OPS_TREASURY_ADDRESS=0x…            # 用于地址校验
```

**aws-kms backend:**

```
AWS_REGION=us-east-1
KMS_KEY_ID_OWNER_A=arn:aws:kms:us-east-1:…:key/…
OWNER_A_ADDRESS=0x…
KMS_ASSUME_ROLE_ARN=arn:aws:iam::…:role/cyberclaw-signer
```

**ledger backend:**

设备需要物理插入操作员机器。`require_physical_confirmation = true` 表示
每次签名都要按硬件按键。

## Capability 详细规格

### `signer.list` (Low)

```json
{}
```

返回（**不包含任何密钥材料**）：

```json
{
  "signers": [
    { "id": "ops-treasury-1",      "backend": "keystore", "address": "0x…" },
    { "id": "ops-multisig-owner-A","backend": "aws-kms",  "address": "0x…" },
    { "id": "ops-relay-1",         "backend": "keystore", "address": "0x…" },
    { "id": "hardware-cold",       "backend": "ledger",   "address": "0x…" }
  ]
}
```

### `signer.sign.tx.eip1559` (High, 1 approval)

```json
{
  "signer_id": "ops-treasury-1",
  "envelope": {
    "chain_id": 1,
    "to": "0x…",
    "value_wei": "100000000000000000",
    "data": "0x…",
    "max_fee_per_gas": "30000000000",
    "max_priority_fee_per_gas": "1500000000",
    "gas_limit": 21000,
    "nonce": 42
  }
}
```

行为：
1. PolicyEngine 评估 + 1 个审批
2. 校验 signer 的链上地址与 manifest 注册值匹配（防 keystore 被替换）
3. 调 backend 签名
4. 返回 raw signed tx；**不广播**（广播是 wallet-eth 的职责）

返回：

```json
{
  "raw_signed_tx": "0x02f8…",
  "tx_hash": "0x…",
  "from": "0x…",
  "signer_id": "ops-treasury-1"
}
```

### `signer.sign.typed.eip712` (High, 1 approval)

输入 EIP-712 typed data：

```json
{
  "signer_id": "ops-multisig-owner-A",
  "domain": {
    "name": "Safe",
    "version": "1.4.1",
    "chainId": 1,
    "verifyingContract": "0xSafe…"
  },
  "types": {
    "SafeTx": [
      { "name": "to", "type": "address" },
      { "name": "value", "type": "uint256" },
      …
    ]
  },
  "primaryType": "SafeTx",
  "message": { … }
}
```

返回：

```json
{
  "signature": "0x…",
  "digest": "0x…",
  "signer_address": "0x…"
}
```

## 安全提示

- **`signer.sign.raw.bytes` 默认关闭**。如果某业务非要打开，建议在
  `governance.toml` 里加 iron-law 规则限定：调用方只能是某个特定
  `connector_id`，且 `payload` 必须先经过 `prefix_check`。
- **strict_payload_classification = true**: vault 会主动识别"raw bytes"
  里看起来像 EIP-712 / EIP-191 / RLP 编码 tx 的 payload，并拒绝调用——
  强制使用对应 typed capability。
- **verify_address_before_sign = true**: 每次签名前重新派生地址，跟注册
  时声明的地址比对。防 keystore 文件被替换 / KMS key 被切。
- **signature_rate_anomaly**: 短时间内同一 signer 大量签名会发 security
  event，触发观察 / 限流 / 自动暂停。

## 与上游 connector 协作流程

```
[Agent]                  [wallet-eth]              [signer-vault]
   │                          │                         │
   │ "transfer 1 ETH" ──►     │                         │
   │                    ┌─ wallet.eth.tx.simulate (Low) │
   │                    │  ✓                            │
   │                    │                               │
   │                    └─ wallet.eth.tx.sign (Crit)    │
   │                          │ ──► signer.sign.tx       │
   │                          │      .eip1559 (High)    │
   │                          │      ┌──► verify addr   │
   │                          │      ├──► PolicyEngine  │
   │                          │      ├──► approval      │
   │                          │      ├──► backend.sign  │
   │                          │      └──── raw_tx ──┐   │
   │                          │ ◄────────────────── ┘   │
   │                    ┌─ wallet.eth.tx.broadcast (Crit)
   │                    │     ✓ submitted              │
```

每一跳是独立审计行；vault 自己也会落审计行（"signer X used at T for
payload-hash Y by caller Z"）。

## 实现状态

manifest + capability 规格在本目录，参考实现在
`crates/cyberclaw-connectors/src/web3/signer/`（v0.2.0 落地，按 backend
分文件）。当前注册到 registry 后治理链立即生效；调用 `signer.sign.*` 在
Rust handler 合并前会返回 `Unimplemented`。
