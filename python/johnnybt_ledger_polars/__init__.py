"""An amount-based bookkeeping policy as a polars expression plugin.

`simulate` walks one account per group under this policy — the same
contract as `johnnybt_polars.simulate`, a different policy behind it:
decisions are funded by netting, the sellable budget is money rather than
share counts, and sizing runs twice around the release of frozen cash.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Any

import polars as pl
from polars.plugins import register_plugin_function

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

_LIB = Path(__file__).parent


def simulate(columns: Sequence[pl.Expr | str], **kwargs: Any) -> pl.Expr:
    """Walk one account per group under the amount-based bookkeeping.

    Args:
        columns: The list and scalar columns, in the order the kwargs index.
        **kwargs: What each column is, the market's rules and the account's
            terms — the `SimulateKwargs` contract. `Any`: the kwargs are
            serialised for the plugin; each key's shape is fixed there.

    Returns:
        The expression: one struct per bar (equity, cash, fees, bought, sold).
    """
    return register_plugin_function(
        plugin_path=_LIB, args=list(columns), function_name="simulate", is_elementwise=False, kwargs=dict(kwargs)
    )
