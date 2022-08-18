use std::{marker::PhantomData, borrow::Borrow};

use ark_ff::PrimeField;
use ark_r1cs_std::{fields::fp::FpVar, prelude::Boolean, uint64::UInt64, uint8::UInt8, ToBitsGadget};
use ark_relations::r1cs::{SynthesisError, ConstraintSystemRef};
use ark_sponge::{
    constraints::CryptographicSpongeVar,
    poseidon::{
        PoseidonParameters, 
        constraints::PoseidonSpongeVar
    },
};

use super::{types::WalletVar, constants::{POSEIDON_MDS_MATRIX_T_3, POSEIDON_ROUND_CONSTANTS_T_3}, wallet_match::MatchResultVariable};


/**
 * Helpers
*/

// Converts a UInt64 constraint variable into a field element
fn u64_to_field_element<F: PrimeField>(a: &UInt64<F>) -> Result<FpVar<F>, SynthesisError> {
    Boolean::le_bits_to_fp_var(&a.to_bits_le())
}

fn u8_to_field_element<F: PrimeField>(a: &UInt8<F>) -> Result<FpVar<F>, SynthesisError> {
   Boolean::le_bits_to_fp_var(&a.to_bits_le()?)
}

/**
 * Constraint system types
 */

pub trait PoseidonHashInput<F: PrimeField> {
    fn get_elements(&self) -> &Vec<FpVar<F>>;
}

// The order hash input only hashes the orders of a given wallet
#[derive(Debug)]
pub struct OrderHashInput<F: PrimeField> {
    elements: Vec<FpVar<F>>,
}

impl<F: PrimeField> PoseidonHashInput<F> for OrderHashInput<F> {
    fn get_elements(&self) -> &Vec<FpVar<F>> {
        &self.elements
    }
}

// Allocate an order hash input from the wallet input
impl<F: PrimeField> From<&WalletVar<F>> for Result<OrderHashInput<F>, SynthesisError> {
    fn from(wallet: &WalletVar<F>) -> Self {
        let mut elements = Vec::<FpVar<F>>::new();
        wallet.borrow()
            .orders
            .iter()
            .map(|order| {
                elements.push(u64_to_field_element(&order.base_mint)?);
                elements.push(u64_to_field_element(&order.quote_mint)?);
                elements.push(u8_to_field_element(&order.side)?);
                elements.push(u64_to_field_element(&order.price)?);
                elements.push(u64_to_field_element(&order.amount)?);
                Ok(())
            })
            .collect::<Result<Vec<()>, SynthesisError>>()?;
        
        Ok(OrderHashInput { elements })
    }
}

// The wallet hash input hashes the entire wallet
#[derive(Debug)]
pub struct WalletHashInput<F: PrimeField> {
    elements: Vec<FpVar<F>>
}

impl<F: PrimeField> PoseidonHashInput<F> for WalletHashInput<F> {
    fn get_elements(&self) -> &Vec<FpVar<F>> {
        &self.elements
    }
}

// Allocate a wallet hash input from the wallet itself
impl<F: PrimeField> From<&WalletVar<F>> for Result<WalletHashInput<F>, SynthesisError> {
    fn from(wallet: &WalletVar<F>) -> Self {
        let mut elements = Vec::<FpVar<F>>::new();
        let wallet = wallet.borrow();

        // Add all balances to the hash input
        wallet.balances
            .iter()
            .map(|balance| {
                elements.push(u64_to_field_element(&balance.amount)?);
                elements.push(u64_to_field_element(&balance.mint)?);
                Ok(())
            })
            .collect::<Result<Vec<()>, SynthesisError>>()?;
        
        // Add all orders to the hash input
        wallet.orders
            .iter()
            .map(|order| {
                elements.push(u64_to_field_element(&order.base_mint)?);
                elements.push(u64_to_field_element(&order.quote_mint)?);
                elements.push(u8_to_field_element(&order.side)?);
                elements.push(u64_to_field_element(&order.price)?);
                elements.push(u64_to_field_element(&order.amount)?);
                Ok(())
            })
            .collect::<Result<Vec<()>, SynthesisError>>()?;
        
        Ok( WalletHashInput { elements } )
         
    }
}

// The match hash input hashes two lists of matches
#[derive(Debug)]
pub struct MatchHashInput<F: PrimeField> {
    elements: Vec<FpVar<F>>
}

impl<F: PrimeField> PoseidonHashInput<F> for MatchHashInput<F> {
    fn get_elements(&self) -> &Vec<FpVar<F>> {
        &self.elements
    }
}

impl<F: PrimeField> From<&MatchResultVariable<F>> for Result<MatchHashInput<F>, SynthesisError> {
    fn from(match_res: &MatchResultVariable<F>) -> Self {
        let mut elements = Vec::<FpVar<F>>::new();
        match_res.matches1
            .iter()
            .chain(match_res.matches2.iter())
            .map(|match_var| {
                elements.push(u64_to_field_element(&match_var.amount)?);
                elements.push(u64_to_field_element(&match_var.mint)?);
                elements.push(u8_to_field_element(&match_var.side)?);
                Ok(())
            })
            .collect::<Result<Vec<()>, SynthesisError>>()?;
        
        Ok( MatchHashInput { elements } )
        
    }
}

// A thin wrapper over the PoseidonSpongeVar that implements a default config
#[derive(Clone)]
pub struct PoseidonSpongeWrapperVar<F: PrimeField> {
    pub sponge: PoseidonSpongeVar<F>
}

impl<F: PrimeField> PoseidonSpongeWrapperVar<F> {
    pub fn new(
        cs: ConstraintSystemRef<F>
    ) -> Self {
        PoseidonSpongeWrapperVar {
            sponge: PoseidonSpongeVar::new(cs, &Self::default_params())
        }
    }

    pub fn default_params() -> PoseidonParameters<F> {
        // Poseidon parameters taken from the paper (tables 2 and 8): https://eprint.iacr.org/2019/458.pdf
        // and generated from the Hades scripts here: https://extgit.iaik.tugraz.at/krypto/hadeshash
        // t = 3 here for a 2 to 1 hash (to be composed into a Merkle tree)
        PoseidonParameters::new(
            8 /* full_rounds */,				// Number of full S box rounds to apply before and after partial rounds
            56 /* partial_rounds */,			        // Number of partial S box rounds, applying the S-box to only one element
            5 /* alpha */,					// \alpha from the paper; used to parameterize the permutation y = x^\alpha (mod p)
            POSEIDON_MDS_MATRIX_T_3::<F>() /* mds */,	        // The MDS matrix used as a linear layer after the S-box
            POSEIDON_ROUND_CONSTANTS_T_3::<F>() /* ark */,	// The round constants for each round, xor'd with the input to each round
        )
    }
}

/**
 * Gadgets
 */
pub struct PoseidonVectorHashGadget<F: PrimeField> {
    _phantom: PhantomData<F>,
}

impl<F: PrimeField> PoseidonVectorHashGadget<F> {
    pub fn evaluate(
        input: &impl PoseidonHashInput<F>,
        poseidon_sponge: &mut PoseidonSpongeWrapperVar<F>
    ) -> Result<FpVar<F>, SynthesisError> {
        // Assume the input to be of length 2^n for some n 
        for next_elem in input.get_elements().iter() {
            poseidon_sponge.sponge.absorb(next_elem)?;
        }

        Ok(
            poseidon_sponge.sponge.squeeze_field_elements(1)?
                .pop()
                .ok_or(SynthesisError::AssignmentMissing)?
        )
    }
}

#[cfg(test)]
mod test {
    use ark_r1cs_std::{prelude::{AllocVar, EqGadget}, fields::fp::FpVar, R1CSVar};
    use ark_relations::r1cs::{ConstraintSystem, SynthesisError};

    use crate::circuits::{types::{Wallet, Balance, Order, OrderSide, WalletVar}, SystemField, poseidon::OrderHashInput};

    use super::{WalletHashInput, PoseidonSpongeWrapperVar, PoseidonVectorHashGadget};

    #[test]
    fn test_order_hash() {
        let wallet = Wallet {
            balances: vec![ Balance { mint: 1, amount: 10 } ],
            orders: vec![ Order { quote_mint: 1, base_mint: 2, side: OrderSide::Buy, amount: 1, price: 10 } ]
        };

        let expected_hash = wallet.hash_orders();

        // Build a constraint system and hash within the cs
        let cs = ConstraintSystem::<SystemField>::new_ref();

        let wallet_var = WalletVar::new_input(cs.clone(), || Ok(wallet)).unwrap();

        let mut wallet_hasher = PoseidonSpongeWrapperVar::new(cs.clone());
        let hash_input = Result::<OrderHashInput<SystemField>, SynthesisError>::from(&wallet_var).unwrap();

        let wallet_hash = PoseidonVectorHashGadget::evaluate(&hash_input, &mut wallet_hasher).unwrap();

        assert!(
            wallet_hash.is_eq(&FpVar::Constant(SystemField::from(expected_hash)))
                .unwrap()
                .value()
                .unwrap()
        );
    }

    #[test]
    fn test_wallet_hash() {
        let wallet = Wallet {
            balances: vec![ Balance { mint: 1, amount: 10 } ],
            orders: vec![ Order { quote_mint: 1, base_mint: 2, side: OrderSide::Buy, amount: 1, price: 10 } ]
        };

        let expected_hash = wallet.hash();

        // Build a constraint system and hash within the cs
        let cs = ConstraintSystem::<SystemField>::new_ref();

        let wallet_var = WalletVar::new_input(cs.clone(), || Ok(wallet)).unwrap();

        let mut wallet_hasher = PoseidonSpongeWrapperVar::new(cs.clone());
        let hash_input = Result::<WalletHashInput<SystemField>, SynthesisError>::from(&wallet_var).unwrap();

        let wallet_hash = PoseidonVectorHashGadget::evaluate(&hash_input, &mut wallet_hasher).unwrap();

        assert!(
            wallet_hash.is_eq(&FpVar::Constant(SystemField::from(expected_hash)))
                .unwrap()
                .value()
                .unwrap()
        )
    }
}