# Reentrancy Guards

## Why this exists

Reentrancy is the #1 historically catastrophic Solidity bug class:
- **The DAO (2016)**: ~$60M drained → ETH/ETC hard fork.
- **Cream Finance (2021)**: $130M.
- **Fei Protocol (2022)**: $80M.
- **Curve (2023)**: $73M (Vyper compiler bug in reentrancy lock).

Pattern recognition: any function that performs an external call before
finalising state is a candidate.

## Single-function reentrancy

```solidity
// VULNERABLE
mapping(address => uint256) public balances;

function withdraw() external {
    uint256 amount = balances[msg.sender];
    require(amount > 0);
    (bool ok,) = msg.sender.call{value: amount}("");
    require(ok);
    balances[msg.sender] = 0;   // <-- after external call
}
```

Attacker receives ETH in their `receive()`, which re-calls `withdraw()`. Repeats
until contract is drained.

### Fix: Checks-Effects-Interactions (CEI)

```solidity
function withdraw() external {
    uint256 amount = balances[msg.sender];   // CHECK
    require(amount > 0);
    balances[msg.sender] = 0;                // EFFECT
    (bool ok,) = msg.sender.call{value: amount}("");  // INTERACTION
    require(ok);
}
```

The state is zeroed before the external call. Re-entry sees `balances[msg.sender] == 0` and the `require` fails.

### Fix: nonReentrant modifier

```solidity
import {ReentrancyGuard} from "@openzeppelin/contracts/security/ReentrancyGuard.sol";

contract Bank is ReentrancyGuard {
    function withdraw() external nonReentrant { ... }
}
```

OZ's implementation:
- Two storage slots: `_NOT_ENTERED = 1`, `_ENTERED = 2`.
- On entry: require state == NOT_ENTERED, set to ENTERED.
- On exit: set back to NOT_ENTERED.
- Cost: ~5,000 gas first call (cold SSTORE), ~2,300 subsequent (warm).

EIP-1153 (Cancun, March 2024) variant uses transient storage (`tstore`/`tload`) which auto-resets per tx — cheaper (~200 gas).

## Cross-function reentrancy

```solidity
// Two functions share state
function deposit() external payable { balances[msg.sender] += msg.value; }
function withdraw() external nonReentrant { ... }
function transferBalance(address to) external {
    balances[to] += balances[msg.sender];
    balances[msg.sender] = 0;
}
```

If `withdraw` is nonReentrant but `transferBalance` is not, the callback in
`withdraw` can call `transferBalance` to clone the balance to an EOA before
`withdraw`'s state update completes.

**Fix**: apply `nonReentrant` to **all** functions sharing the same critical
state, not just the "obvious" one. ReentrancyGuard's lock is per-contract,
not per-function — once entered, no nonReentrant function is callable.

## Read-only reentrancy

```solidity
// Lender contract reads getCurrentPrice() from Curve pool
function liquidate(address borrower) external {
    uint256 price = curvePool.get_virtual_price();
    // ... use price for liquidation math
}
```

Attacker triggers a Curve `remove_liquidity` that calls back into the
attacker's `receive()`. Inside `receive()`, Curve's reentrancy lock is held but
read-only views (`get_virtual_price`) return mid-state values that are
mathematically wrong. Attacker calls `lender.liquidate()` from that callback
window and exploits the wrong price.

**Fix**:
1. Curve / Balancer now expose `is_locked()` or equivalent; consumers check
   and revert if true.
2. Wrap any oracle read that goes to a pool with a reentrancy guard on the
   consumer side: `nonReentrant` modifier on `liquidate()` so the outer call
   path can't be entered via the attacker's callback.

## Cross-contract reentrancy

When state lives in a separate contract (e.g. `Vault` + `Strategy`), the
reentrancy lock on `Vault` does not protect `Strategy`. Attack path:

1. `Vault.deposit()` calls `Strategy.allocate(amount)`.
2. `Strategy.allocate` calls an external swap router.
3. Router callback re-enters `Vault.withdraw()` — Vault's lock is held, so
   this reverts. Good.
4. But the callback re-enters `Strategy.report()`, which is unlocked. It
   reads `Vault.totalAssets()` which is in a half-updated state, and writes
   bad accounting to `Strategy`.

**Fix**: every external-facing function on every contract sharing state must
be `nonReentrant`. Or: use a single shared `Pausable` + `ReentrancyGuard`
inherited base for the whole protocol cluster.

## Pull-payment pattern

Avoid pushing ETH or tokens during state-changing functions. Instead, mark
amounts as "claimable":

```solidity
mapping(address => uint256) public pendingWithdrawals;

function requestWithdraw(uint256 amount) external {
    require(balances[msg.sender] >= amount);
    balances[msg.sender] -= amount;          // EFFECT immediately
    pendingWithdrawals[msg.sender] += amount;
}

function claim() external nonReentrant {
    uint256 amount = pendingWithdrawals[msg.sender];
    require(amount > 0);
    pendingWithdrawals[msg.sender] = 0;       // CEI
    (bool ok,) = msg.sender.call{value: amount}("");
    require(ok);
}
```

The external call is isolated to a function that has no other state. Lowest
risk surface.

## Audit checklist

For each function in a contract:

1. Does it make an external call (`call`, `delegatecall`, `transfer`,
   `staticcall`, `ERC20.transferFrom`)?
2. If yes, is there any state mutation **after** the call?
3. Is the function `nonReentrant`?
4. Are all other functions that share the same critical state also
   `nonReentrant`?
5. Are any view functions read by external integrators during a state
   transition (read-only reentrancy)?

If any answer is "no" or "unsure", flag for re-design.

## Tools

- **Slither** (Trail of Bits): static analysis, has `reentrancy-eth`,
  `reentrancy-no-eth`, `reentrancy-events` detectors.
- **Mythril** (ConsenSys): symbolic execution, slower but finds deeper
  paths.
- **Echidna** (Trail of Bits): property-based fuzzing; write a property
  "balance can only decrease in withdraw" and let it fuzz attack paths.
- **Foundry's `forge test --gas-report` + invariant tests**: write
  invariants and run with `inv` budget; Foundry generates random call
  sequences including reentrant ones.
