//! How this policy keeps its books, operation for operation.
//!
//! Every rule was settled by reconciling against a production ledger day
//! by day, and the comments name the case that established each one. What
//! it adds to the framework's plain policy:
//!
//! * **Netted deposits and a pocket per tranche.** A decision is funded
//!   once, at sizing, with the gap between target equity and holdings on
//!   the assumption its sells go through. Fills carry no cash check; a
//!   refused sale leaves the buys it was meant to fund unpaid, and the debt
//!   lives in the tranche's own balance — outside the account's cash and
//!   the reported equity — until its next net sale repays it.
//! * **A money-like sellable budget, account-wide.** One per name across
//!   every tranche and the pool, reset at each close to everything held,
//!   drawn down by what each decision *meant* to sell (the mask pass) and
//!   only otherwise by a refused reduction's partial fill.
//! * **Two sizing passes.** The first finds what settlement refuses; the
//!   decision then withdraws its frozen orders and the second sizes what
//!   actually trades, a refused name keeping the weight the book attained.
//! * **Cash frozen behind a blocked buy**, at the decision's own size plus
//!   its estimated commission, retried at every later point and cancelled
//!   at the close.

use johnnybt_engine::account::{merged, Account};
use johnnybt_engine::bookkeeping::Bookkeeping;
use johnnybt_engine::inputs::*;

pub struct Xbx {
    tranches: usize,
    assets: usize,
    /// `(N,)` the account's sellable budget, carried in money.
    free: Vec<f64>,
    /// `(K, N)` cash frozen behind a blocked buy, by the cell that froze it.
    frozen_by: Vec<f64>,
    /// The bar's frozen total.
    frozen: f64,
    /// `(K,)` each tranche's pocket: its balance after settlement, never
    /// positive.
    pocket: Vec<f64>,
    /// The names whose budget is non-zero, in no order: what the daily
    /// reset has to clear before it sets the day's budget.
    budgeted: Vec<usize>,
    /// `(N,)` whether a name is in `budgeted`.
    is_budgeted: Vec<bool>,
}

impl Xbx {
    #[inline]
    fn at(&self, tranche: usize, asset: usize) -> usize {
        tranche * self.assets + asset
    }

    /// Settlement: the account takes back only the positive part; the debt
    /// stays in the tranche's pocket — the withdrawal floors at
    /// zero.
    fn settle(&mut self, acct: &mut Account, tranche: usize, simcash: f64) {
        if simcash > 0.0 {
            acct.cash += simcash;
            self.pocket[tranche] = 0.0;
        } else {
            self.pocket[tranche] = simcash;
        }
    }

    /// The mask pass: size under the standing state, find the refusals,
    /// spend the budget by intent.
    fn mask_pass(
        &mut self,
        acct: &mut Account,
        inp: &Inputs,
        tranche: usize,
        decision: usize,
        at: Point,
    ) {
        let run = acct.run;
        let row = acct.row_names(inp, tranche, decision);
        let mut share = 0.0f64;
        for &asset in row.iter() {
            let weight = inp.plan[(run, tranche, decision, asset)];
            if !weight.is_nan() {
                share += weight.abs();
            }
        }
        // Free cash only: money frozen behind a blocked buy is spoken for —
        // on this pass that includes the tranche's own standing budget.
        let reachable = acct.holding(inp, tranche, at) + acct.cash - self.frozen;
        let investable = acct.investable(inp, share, reachable);
        // The row's names and the book's; any other name strikes zero on a
        // zero weight, holds nothing, and its budget check is 0 > free.
        let names = merged(&row, acct.named(tranche));
        for &asset in names.iter() {
            let want = acct.strike(
                inp,
                tranche,
                asset,
                inp.plan[(run, tranche, decision, asset)],
                investable,
                at,
            );
            let cell = acct.at(tranche, asset);
            acct.target[cell] = want;
            if want != 0.0 {
                acct.touch(tranche, asset);
            }
        }
        // Read before trading — and **spent here, by intent**: the mask
        // pass subtracts each legal sell's delta from the budget before
        // anything trades, and the trade pass never subtracts again. The two passes size under different cash (the
        // frozen budget is withdrawn between them), so what the mask pass
        // wanted to sell — not what the trade pass sells — is what the
        // budget loses (2023-06-29 13:05, sh603536).
        for &asset in names.iter() {
            let cell = acct.at(tranche, asset);
            let give = acct.qty[cell] - acct.target[cell];
            acct.blocked[cell] = inp.audit && give > self.free[asset];
            if acct.blocked[cell] {
                acct.touch(tranche, asset);
            }
            if inp.audit && give > 0.0 && !acct.blocked[cell] {
                self.free[asset] -= give;
            }
        }
    }

    /// The trade pass: withdraw the standing frozen orders, re-size with a
    /// refused name held at what the book attained, fund the decision
    /// netted, then sell and buy on the tranche's own balance.
    fn trade_pass(
        &mut self,
        acct: &mut Account,
        inp: &Inputs,
        tranche: usize,
        decision: usize,
        at: Point,
    ) {
        for &asset in acct.named(tranche) {
            let cell = self.at(tranche, asset);
            if self.frozen_by[cell] > 0.0 {
                self.frozen -= self.frozen_by[cell];
                self.frozen_by[cell] = 0.0;
            }
        }
        let names = merged(&acct.row_names(inp, tranche, decision), acct.named(tranche));
        let mut share = 0.0f64;
        for &asset in names.iter() {
            share += self
                .weight_of(acct, inp, tranche, decision, asset)
                .abs_or_zero();
        }
        let deposit: f64;
        let investable = if share < 1e-12 {
            deposit = 0.0;
            0.0
        } else {
            let holding = acct.holding(inp, tranche, at);
            let reachable = holding + acct.cash - self.frozen;
            let mut target_equity = acct.equity * share;
            if reachable < target_equity {
                target_equity = reachable;
            }
            // The decision's netted funding: what the targets need beyond
            // the holdings, on the assumption the sells go through.
            let funding = target_equity - holding;
            deposit = if funding < 0.0 { 0.0 } else { funding };
            target_equity * inp.buffer / share
        };
        for &asset in names.iter() {
            let weight = self.weight_of(acct, inp, tranche, decision, asset);
            let want = acct.strike(inp, tranche, asset, weight, investable, at);
            let cell = acct.at(tranche, asset);
            acct.target[cell] = want;
            if want != 0.0 {
                acct.touch(tranche, asset);
            }
        }
        // What the settlement refuses, it refuses in both directions: the
        // name sells everything sellable and buys nothing, whatever the
        // re-sizing would have asked for.
        for idx in 0..acct.named(tranche).len() {
            let asset = acct.named(tranche)[idx];
            let cell = acct.at(tranche, asset);
            if acct.blocked[cell] {
                acct.target[cell] = acct.qty[cell] - self.free[asset];
            }
        }
        // The tranche trades on its own balance: pocket debt first, then
        // the decision's netted deposit.
        acct.cash -= deposit;
        let mut simcash = self.pocket[tranche] + deposit;
        simcash = self.sell(acct, inp, tranche, at, simcash);
        simcash = self.buy(acct, inp, tranche, at, simcash, false);
        self.settle(acct, tranche, simcash);
        acct.remember_attained(inp, tranche, decision);
    }

    /// The weight the trade pass sizes a name at: the plan's, or the
    /// attained one where the reduction was refused.
    #[inline]
    fn weight_of(
        &self,
        acct: &Account,
        inp: &Inputs,
        tranche: usize,
        decision: usize,
        asset: usize,
    ) -> f64 {
        let cell = acct.at(tranche, asset);
        let weight = inp.plan[(acct.run, tranche, decision, asset)];
        if acct.blocked[cell] && !acct.attained[cell].is_nan() {
            acct.attained[cell]
        } else {
            weight
        }
    }

    /// This tranche's sells at this point, paid into its balance.
    fn sell(
        &mut self,
        acct: &mut Account,
        inp: &Inputs,
        tranche: usize,
        at: Point,
        mut simcash: f64,
    ) -> f64 {
        let (bar, phase, epoch) = (at.bar, at.phase, at.epoch);
        for idx in 0..acct.named(tranche).len() {
            let asset = acct.named(tranche)[idx];
            let price = inp.price(phase, bar, asset);
            if price.is_nan() || price <= 0.0 {
                continue;
            }
            let cell = acct.at(tranche, asset);
            let mut give = acct.qty[cell] - acct.target[cell];
            if give <= 0.0 {
                continue;
            }
            if !inp.sellable(phase, bar, asset) {
                continue;
            }
            // Only a refused reduction is held to what may be sold: the
            // settlement check belongs to sizing, not to the fill.
            if acct.blocked[cell] && give > self.free[asset] {
                give = self.free[asset];
            }
            // No lot rounding on the way out: a corporate action leaves a
            // fractional share count, and flooring every sell strands it.
            if give <= 0.0 {
                continue;
            }
            let category = inp.class(asset);
            let turnover = give * price * inp.rate(MULTIPLIER, epoch, category);
            let fee = inp.fee(
                turnover,
                inp.rate(SELL_FEE, epoch, category),
                epoch,
                category,
            );
            acct.qty[cell] -= give;
            // Only a refused reduction draws the budget down here — a legal
            // sell already spent it in the mask pass, by its intent.
            if acct.blocked[cell] {
                self.free[asset] -= give;
            }
            simcash += turnover - fee;
            acct.fees += fee;
            acct.sold += turnover;
            acct.fill(at, tranche as i32, asset, -give, price, fee);
        }
        simcash
    }

    /// This tranche's buys at this point, drawn from its balance without a
    /// cash check. With `pending_only`, only the names with a frozen budget
    /// try — the backfill of a blocked order.
    fn buy(
        &mut self,
        acct: &mut Account,
        inp: &Inputs,
        tranche: usize,
        at: Point,
        mut simcash: f64,
        pending_only: bool,
    ) -> f64 {
        let (bar, phase, epoch) = (at.bar, at.phase, at.epoch);
        for idx in 0..acct.named(tranche).len() {
            let asset = acct.named(tranche)[idx];
            let cell = acct.at(tranche, asset);
            if pending_only && self.frozen_by[cell] <= 0.0 {
                continue;
            }
            let price = inp.price(phase, bar, asset);
            if price.is_nan() || price <= 0.0 {
                continue;
            }
            let category = inp.class(asset);
            let take = acct.target[cell] - acct.qty[cell];
            if take <= 0.0 {
                continue;
            }
            let unit = price * inp.rate(MULTIPLIER, epoch, category);
            let rate = inp.rate(BUY_FEE, epoch, category);
            if !inp.buyable(phase, bar, asset) {
                // Blocked at the limit: the order's budget freezes, at the
                // decision's own size — the full delta plus its estimated
                // commission, unclamped.
                if !pending_only && self.frozen_by[cell] == 0.0 {
                    let wanted_cash = take * unit * (1.0 + rate);
                    self.frozen_by[cell] = wanted_cash;
                    self.frozen += wanted_cash;
                }
                continue;
            }
            if self.frozen_by[cell] > 0.0 {
                // The pending budget leaves the account for the balance;
                // whatever the fill does not spend flows back at settlement.
                let budget = self.frozen_by[cell];
                self.frozen -= budget;
                self.frozen_by[cell] = 0.0;
                acct.cash -= budget;
                simcash += budget;
            }
            if acct.qty[cell] + take < inp.rate(MIN_LOT, epoch, category) {
                continue;
            }
            let turnover = take * unit;
            let fee = inp.fee(turnover, rate, epoch, category);
            acct.qty[cell] += take;
            if inp.flags[(SAME_BAR, category)] {
                self.free[asset] += take;
                self.budget_named(asset);
            }
            simcash -= turnover + fee;
            acct.fees += fee;
            acct.bought += turnover;
            acct.fill(at, tranche as i32, asset, take, price, fee);
        }
        simcash
    }
}

impl Xbx {
    /// Note that name `i` carries a budget the daily reset must clear.
    #[inline]
    fn budget_named(&mut self, asset: usize) {
        if !self.is_budgeted[asset] {
            self.is_budgeted[asset] = true;
            self.budgeted.push(asset);
        }
    }
}

trait AbsOrZero {
    fn abs_or_zero(self) -> f64;
}

impl AbsOrZero for f64 {
    /// |w| for a weight, nothing for a missing one.
    #[inline]
    fn abs_or_zero(self) -> f64 {
        if self.is_nan() {
            0.0
        } else {
            self.abs()
        }
    }
}

impl Bookkeeping for Xbx {
    fn new(tranches: usize, assets: usize) -> Self {
        Xbx {
            tranches,
            assets,
            free: vec![0.0; assets],
            frozen_by: vec![0.0; tranches * assets],
            frozen: 0.0,
            pocket: vec![0.0; tranches],
            budgeted: Vec::new(),
            is_budgeted: vec![false; assets],
        }
    }

    fn corporate_action(&mut self, _acct: &Account, asset: usize, ratio: f64) {
        if self.free[asset] != 0.0 {
            self.free[asset] *= ratio;
        }
    }

    fn new_day(&mut self, acct: &Account) {
        // The budget resets to everything the account holds — every tranche
        // and the liquidation pool alike, the close-of-day sum over all
        // of the account's pockets. Any other name's budget is zero: cleared
        // for the names that carried one, never scanned for the rest.
        for asset in std::mem::take(&mut self.budgeted) {
            self.free[asset] = 0.0;
            self.is_budgeted[asset] = false;
        }
        for asset in acct.union_names() {
            self.free[asset] = acct.held(asset);
            if self.free[asset] != 0.0 {
                self.budget_named(asset);
            }
        }
    }

    fn open_bar(&mut self) {
        self.frozen = 0.0;
    }

    fn backfill(&mut self, acct: &mut Account, inp: &Inputs, at: Point) {
        // The pending orders try first, at their frozen share counts: the
        // backfill scan runs at the top of every price point, before the
        // equity snapshot and the decision loops.
        for tranche in 0..self.tranches {
            let mut waiting = false;
            for &asset in acct.named(tranche) {
                if self.frozen_by[self.at(tranche, asset)] > 0.0 {
                    waiting = true;
                    break;
                }
            }
            if !waiting {
                continue;
            }
            let simcash = self.buy(acct, inp, tranche, at, self.pocket[tranche], true);
            self.settle(acct, tranche, simcash);
        }
    }

    fn decide_and_trade(&mut self, acct: &mut Account, inp: &Inputs, at: Point) {
        // **The order of the passes is the audit's.** With the audit on,
        // the mask pass runs for every tranche before anything trades and
        // the trading follows tranche by tranche; without it the two
        // interleave per tranche.
        let mut order: Vec<(u8, usize)> = Vec::with_capacity(self.tranches * 2);
        if inp.audit {
            for tranche in 0..self.tranches {
                order.push((0u8, tranche));
            }
            for tranche in 0..self.tranches {
                order.push((1u8, tranche));
            }
        } else {
            for tranche in 0..self.tranches {
                order.push((0u8, tranche));
                order.push((1u8, tranche));
            }
        }
        for (which, tranche) in order {
            let decision = acct.fire[tranche];
            if decision < 0 {
                continue;
            }
            let decision = decision as usize;
            if which == 0 {
                self.mask_pass(acct, inp, tranche, decision, at);
            } else {
                self.trade_pass(acct, inp, tranche, decision, at);
            }
        }
    }

    fn close_bar(&mut self, acct: &mut Account) {
        // Unfilled orders cancel, their cash returns.
        if self.frozen > 0.0 {
            for tranche in 0..self.tranches {
                for &asset in acct.named(tranche) {
                    let cell = self.at(tranche, asset);
                    self.frozen_by[cell] = 0.0;
                }
            }
        }
    }

    fn at_rest(&self, tranche: usize, asset: usize) -> bool {
        self.frozen_by[self.at(tranche, asset)] == 0.0
    }
}
