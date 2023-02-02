//! Traits that pallet uses.

use sp_arithmetic::traits::{BaseArithmetic, Unsigned};

/// Interface for constraints on workers (e.g. resources)
pub trait WorkerConstraints<T> {
    /// Does the worker with given specification meet the constraints?
    fn worker_suitable(&self, worker_spec: &T) -> bool;
}

/// Arithmetic traits
pub trait AtLeast64Bit: BaseArithmetic + From<u16> + From<u32> + From<u64> {}
impl<T: BaseArithmetic + From<u16> + From<u32> + From<u64>> AtLeast64Bit for T {}

pub trait AtLeast64BitUnsigned: AtLeast64Bit + Unsigned {}
impl<T: AtLeast64Bit + Unsigned> AtLeast64BitUnsigned for T {}
