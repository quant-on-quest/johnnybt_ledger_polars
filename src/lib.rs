//! An amount-based bookkeeping policy, as a polars expression plugin on
//! johnnybt's account walk.
//!
//! Where the framework's plain policy funds a decision out of cash the
//! account actually has, this one nets: a decision is funded once at
//! sizing on the assumption its sells go through, and what a refused sale
//! leaves unpaid becomes a debt in the tranche's own pocket. Every rule
//! was settled by reconciling against a production ledger day by day, to
//! the cent. A pure-Python twin holds it bit-for-bit equal by test.

mod bookkeeping;

use johnnybt_engine::plugin::{output_field, simulate as walk, SimulateKwargs};
use polars::prelude::*;
use pyo3::prelude::*;
use pyo3_polars::derive::polars_expr;
use pyo3_polars::PolarsAllocator;

#[global_allocator]
static ALLOC: PolarsAllocator = PolarsAllocator::new();

fn simulate_output(_input_fields: &[Field], kwargs: SimulateKwargs) -> PolarsResult<Field> {
    Ok(output_field("simulate", kwargs.positions, kwargs.fills))
}

/// Walk one account under the amount-based bookkeeping; see
/// `johnnybt_engine::plugin::SimulateKwargs` for the inputs.
#[polars_expr(output_type_func_with_kwargs=simulate_output)]
fn simulate(inputs: &[Series], kwargs: SimulateKwargs) -> PolarsResult<Series> {
    walk::<bookkeeping::Xbx>(inputs, &kwargs)
}

/// The importable half: nothing but the shared library's symbols, which the
/// expression plugin is found through.
#[pymodule]
fn _lib(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
