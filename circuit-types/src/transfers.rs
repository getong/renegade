//! Defines native and circuit types for internal/external transfers
#![allow(missing_docs, clippy::missing_docs_in_private_items)]

// ----------------------
// | External Transfers |
// ----------------------

use circuit_macros::circuit_type;
use curve25519_dalek::{ristretto::CompressedRistretto, scalar::Scalar};
use mpc_bulletproof::r1cs::{LinearCombination, Variable};
use mpc_ristretto::mpc_scalar::scalar_to_u64;
use num_bigint::BigUint;
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::traits::{
    BaseType, CircuitBaseType, CircuitCommitmentType, CircuitVarType, LinearCombinationLike,
};

/// The base external transfer type, not allocated in a constraint system
/// or an MPC circuit
#[circuit_type(serde, singleprover_circuit)]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalTransfer {
    /// The address of the account contract to transfer to/from
    pub account_addr: BigUint,
    /// The mint (ERC20 address) of the token to transfer
    pub mint: BigUint,
    /// The amount of the token transferred
    pub amount: BigUint,
    /// The direction of transfer
    pub direction: ExternalTransferDirection,
}

/// Represents the direction (deposit/withdraw) of a transfer
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ExternalTransferDirection {
    /// Deposit an ERC20 into the darkpool from an external address
    Deposit = 0,
    /// Withdraw an ERC20 from the darkpool to an external address
    Withdrawal,
}

impl BaseType for ExternalTransferDirection {
    fn to_scalars(&self) -> Vec<Scalar> {
        vec![Scalar::from(*self as u8)]
    }

    fn from_scalars<I: Iterator<Item = Scalar>>(i: &mut I) -> Self {
        match scalar_to_u64(&i.next().unwrap()) {
            0 => ExternalTransferDirection::Deposit,
            1 => ExternalTransferDirection::Withdrawal,
            _ => panic!("invalid value for ExternalTransferDirection"),
        }
    }
}

impl CircuitBaseType for ExternalTransferDirection {
    type VarType<L: LinearCombinationLike> = L;
    type CommitmentType = CompressedRistretto;

    fn commitment_randomness<R: RngCore + CryptoRng>(&self, rng: &mut R) -> Vec<Scalar> {
        vec![Scalar::random(rng)]
    }
}

impl Default for ExternalTransferDirection {
    fn default() -> Self {
        Self::Deposit
    }
}

impl From<ExternalTransferDirection> for Scalar {
    fn from(dir: ExternalTransferDirection) -> Self {
        Scalar::from(dir as u8)
    }
}