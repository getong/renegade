//! The note type, used to represent a note spent from one recipients wallet
//! into another, e.g. to transfer a fee

#![allow(missing_docs, clippy::missing_docs_in_private_items)]

use circuit_macros::circuit_type;
use constants::{Scalar, ScalarField};
use mpc_relation::{traits::Circuit, Variable};
use num_bigint::BigUint;
use renegade_crypto::hash::compute_poseidon_hash;
use serde::{Deserialize, Serialize};

use crate::{
    keychain::PublicIdentificationKey,
    traits::{BaseType, CircuitBaseType, CircuitVarType},
    Amount,
};

/// A note allocated into the protocol state by one user transferring to another
#[circuit_type(serde, singleprover_circuit)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The mint of the note
    mint: BigUint,
    /// The amount of the note
    amount: Amount,
    /// The receiver's identification key
    receiver: PublicIdentificationKey,
    /// The blinder of the note
    blinder: Scalar,
}

impl Note {
    /// Compute a commitment to the note
    pub fn commitment(&self) -> Scalar {
        let vals = self.to_scalars();
        compute_poseidon_hash(&vals)
    }
}