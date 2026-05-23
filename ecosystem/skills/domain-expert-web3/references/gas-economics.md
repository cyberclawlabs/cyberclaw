# Gas Economics

## Units (memorise these)

```
1 wei      = 1
1 gwei     = 1e9 wei     = 0.000000001 ETH
1 ether    = 1e18 wei    = 1e9 gwei
```

A "30 gwei" gas price means 30 * 1e9 wei per unit of gas.

## EIP-1559 (London, August 2021)

Before London: simple `gasPrice = bid`. Validator takes all.

After London (every L1 tx is EIP-1559):

```
totalGasPrice = baseFee + min(maxPriorityFeePerGas, maxFeePerGas - baseFee)
cost          = gasUsed * totalGasPrice
```

- `baseFee` is set by the protocol per block. Burned (sent to address(0)).
- `priorityFee` (tip) goes to the validator.
- `maxFeePerGas` is your absolute upper bound. Tx rejected if `baseFee` > `maxFeePerGas`.
- `maxPriorityFeePerGas` is your tip cap.

**baseFee adjustment**:
- If previous block gas used > target (15M gas / 30M cap): `baseFee` rises by up to 12.5 %.
- If < target: `baseFee` falls by up to 12.5 %.
- Geometric → during congestion, fees can multiply 10× in 20 blocks (≈ 4 min).

## Gas tokens

Pre-Berlin (April 2021), `GST2` / `CHI` let you "save" gas at low-fee times
and "spend" it at high-fee times via storage-slot refunds. **Killed by
EIP-3529** (London): max refund capped at 20 % of tx gas. Gas tokens no
longer profitable. Do not use.

## Common gas costs (mainnet, post-Shanghai)

| Operation | Gas |
|---|---|
| Plain ETH transfer (21000 + 0 data) | 21,000 |
| ERC-20 `transfer` (cold) | ~65,000 |
| ERC-20 `transfer` (warm same-block) | ~50,000 |
| ERC-20 `approve` | ~46,000 |
| Uniswap V2 swap (single hop) | ~100,000-130,000 |
| Uniswap V3 swap (single pool) | ~120,000-180,000 |
| Curve stableswap | ~150,000-250,000 |
| NFT mint (ERC-721) | ~80,000-150,000 |
| Safe execTransaction (3/5 sigs) | ~150,000-250,000 |
| Compound supply | ~150,000-200,000 |
| Aave deposit | ~180,000-250,000 |
| Storage write (cold zero → non-zero) | 22,100 (SSTORE) |
| Storage write (non-zero → non-zero, warm) | 100 |
| Storage read (cold) | 2,100 |
| Storage read (warm) | 100 |

## Gas optimisation in Solidity

1. **Pack storage**: variables in same slot if they fit in 256 bits combined.
   ```solidity
   struct PackedBad { uint128 a; uint256 b; uint128 c; }   // 3 slots
   struct PackedGood { uint128 a; uint128 c; uint256 b; }  // 2 slots
   ```
2. **`immutable`** (set in constructor) reads cost 3 gas (PUSH32 inlined),
   not 2,100/100 like storage.
3. **`constant`** even better — no runtime cost.
4. **`calldata` over `memory`** for function args you don't mutate.
   ```solidity
   function f(uint256[] calldata data) external  // cheap
   function f(uint256[] memory data) external    // copies to memory, expensive
   ```
5. **`unchecked { }`** for arithmetic that cannot overflow. Saves ~30 gas
   per op (no underflow check).
6. **Custom errors** instead of `require("message")`.
   ```solidity
   error Insufficient(uint256 have, uint256 want);
   if (have < want) revert Insufficient(have, want);
   // vs require(have >= want, "Insufficient");  -- string is stored, more gas
   ```
7. **`++i` cheaper than `i++`** (no return-old-value temporary in unchecked).
8. **Short-circuit** ordering: cheapest test first.
   ```solidity
   if (someCheapBool && expensiveView(...))   // good
   if (expensiveView(...) && someCheapBool)   // bad if cheap is often false
   ```
9. **Avoid dynamic-length operations in loops**:
   ```solidity
   for (uint256 i; i < arr.length; ++i)   // re-reads length every iter
   uint256 len = arr.length;              // cache it
   for (uint256 i; i < len; ++i)
   ```
10. **Use bitfields** for many booleans:
    ```solidity
    uint256 flags;
    // bit 0: paused, bit 1: emergencyMode, bit 2: feeEnabled...
    ```

## EIP-4844 (Cancun, March 2024) — blob gas

L2 rollups now post their data as "blobs" with a separate fee market.

- Blob is 128 KB of data, lives in beacon chain for ~18 days.
- Blob gas is denominated separately from execution gas.
- `blobBaseFee` follows its own EIP-1559-style market.
- A tx can carry 0..6 blobs.
- Result: L2 fees dropped 10-100× post-Dencun.

If you're operating an L2, you care about both `baseFee` (execution) and
`blobBaseFee` (data). Otherwise it's invisible to L1 contract callers.

## MEV — the silent tax

Every public mempool tx is visible to searchers who can front-run, back-run,
or sandwich. For a treasury Safe doing a 7-figure swap:

- **Sandwich attack**: searcher places a buy tx before yours, your swap moves
  price, searcher sells after. You eat the slippage.
- **Defense 1**: use **Flashbots Protect** (private RPC). Tx submitted via
  `https://rpc.flashbots.net` is not in the public mempool until included.
- **Defense 2**: use **MEV Blocker** or **Cow Swap** (which auctions your
  intent and only executes when sandwiches are unprofitable).
- **Defense 3**: route through a Defender Relayer pointing to a private RPC.
- **Defense 4**: set tight `amountOutMinimum` (max 0.5 % slippage on
  blue-chip pairs). Tx reverts before sandwich profitability.

## Estimating gas before execution

- `eth_estimateGas` — runs the tx against the latest state, returns gas
  needed. Overestimates by ~10 % typically; under heavy state load can
  underestimate.
- Foundry `forge test --gas-report` — for unit-tested paths.
- Tenderly simulation — most accurate, gives gas plus call trace.

Always pad `gasLimit` by 20 % above estimate for production txs. The cost of
a `gasLimit` set too high is zero — unused gas is refunded.

## Stuck transaction recovery

If a tx is pending in mempool but `baseFee` rose above your `maxFeePerGas`:

1. Look up the tx's nonce.
2. Send a **replacement** tx with same nonce, bump both `maxFeePerGas` and
   `maxPriorityFeePerGas` by ≥ 10 % (geth's minimum bump).
3. The replacement can be a no-op (`to = your-address, value = 0`) if you
   want to cancel; or it can be the same tx with higher fees if you want to
   include.

Tools: Etherscan's "Cancel transaction" UI, ethers.js
`provider.send("eth_sendRawTransaction", ...)` with bumped fields.

## Cross-chain gas

- **Arbitrum / Optimism / Base** (Optimistic rollups): pay L1 data cost +
  L2 execution cost. Post-Dencun, L1 cost is now blob-priced; effective L2
  tx fee ~$0.05-0.50.
- **zkSync / Polygon zkEVM / Linea / Scroll** (ZK rollups): similar fee
  structure; prover cost amortised across batches.
- **Polygon PoS** (not a rollup): native MATIC gas, ~$0.01 per tx.

When operating a multichain treasury, hold gas tokens (ETH on Arbitrum,
ETH on Optimism, MATIC on Polygon) in each chain's Safe. A common ops
mistake: deploying a Safe on Arbitrum and forgetting to fund it with ETH;
the first execTransaction will fail with `out of gas`.
