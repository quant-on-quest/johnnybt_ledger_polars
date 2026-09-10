# johnnybt-ledger-polars

An **amount-based bookkeeping policy** for [johnnybt](https://github.com/quant-on-quest/johnnybt)'s
account walk, as a polars expression plugin.

The framework's default policy (`johnnybt-polars`) is market-neutral: a buy only
fills from cash the account actually has, nobody runs a debt. This one keeps the
books the way a production A-share ledger does:

* **Netted funding.** A decision is funded once, at sizing, with the gap between
  target equity and holdings — on the assumption its sells go through. Fills
  carry no cash check; what a refused sale leaves unpaid becomes a debt in that
  tranche's own pocket, outside the account's cash and its reported equity,
  until the next net sale repays it.
* **A sellable budget in money, account-wide.** One per name across every tranche
  and the liquidation pool, reset at each close to everything held, drawn down by
  what each decision *meant* to sell rather than by what it sold.
* **Two-pass sizing.** A mask pass sizes under the tighter cash, the frozen
  budget is released, then the trading pass sizes again — the two see different
  cash on purpose.
* **Estimated commission frozen** with a blocked order, at the share count the
  order was sized for.

It is one `Bookkeeping` implementation of [`johnnybt_engine`](https://github.com/quant-on-quest/johnnybt_engine)'s
account walk: the walk — pricing, ex-dividend, the liquidation pool, equity
snapshots — is the same for every market, and only the funding, sizing and
settlement rules live here.

## Use

```python
import polars as pl
from johnnybt_ledger_polars import simulate

equity = frame.group_by("account").agg(simulate(columns, **kwargs))
```

`columns` and `kwargs` are johnnybt's `SimulateKwargs` contract — which column
plays which part, the market's rules, the account's terms. The same call shape as
`johnnybt_polars.simulate`, a different policy behind it.

## Build

```sh
maturin develop --release      # or: maturin build --release
```

Rust ≥ 1.85, Python ≥ 3.11 (abi3). A pure-Python twin of this policy is held
bit-for-bit equal to it by test, which is how the rules stay honest.

## Licence

MIT
