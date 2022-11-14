//! Groups gadgets around computing Merkle entries and proving Merkle openings

use curve25519_dalek::{ristretto::CompressedRistretto, scalar::Scalar};
use itertools::Itertools;
use mpc_bulletproof::{
    r1cs::{
        LinearCombination, Prover, R1CSProof, RandomizableConstraintSystem, Variable, Verifier,
    },
    r1cs_mpc::R1CSError,
    BulletproofGens,
};
use rand_core::OsRng;

use crate::{mpc_gadgets::poseidon::PoseidonSpongeParameters, SingleProverCircuit};

use super::poseidon::PoseidonHashGadget;

/// The single-prover hash gadget, computes the Merkle root of a leaf given a path
/// of sister nodes
/// TODO: Add path selection variables
pub struct PoseidonMerkleHashGadget {}

impl PoseidonMerkleHashGadget {
    /// Compute the root of the tree given the leaf node and the path of
    /// sister nodes leading to the root
    pub fn compute_root<S, CS>(
        cs: &mut CS,
        leaf_node: Vec<S>,
        opening: Vec<Variable>,
    ) -> Result<LinearCombination, R1CSError>
    where
        S: Into<LinearCombination> + Clone,
        CS: RandomizableConstraintSystem,
    {
        // Hash the leaf_node into a field element
        let mut current_hash = Self::leaf_hash(&leaf_node, cs)?;
        for path_elem in opening.into_iter() {
            current_hash = Self::hash_internal_nodes(&current_hash, &path_elem.into(), cs)?;
        }

        Ok(current_hash)
    }

    /// Compute the root and constrain it to an expected value
    pub fn compute_and_constrain_root<S, CS>(
        cs: &mut CS,
        leaf_node: Vec<S>,
        opening: Vec<Variable>,
        expected_root: S,
    ) -> Result<(), R1CSError>
    where
        CS: RandomizableConstraintSystem,
        S: Into<LinearCombination> + Clone,
    {
        let root = Self::compute_root(cs, leaf_node, opening)?;
        cs.constrain(expected_root.into() - root);

        Ok(())
    }

    /// Hash the value at the leaf into a bulletproof constraint value
    fn leaf_hash<S, CS>(values: &[S], cs: &mut CS) -> Result<LinearCombination, R1CSError>
    where
        S: Into<LinearCombination> + Clone,
        CS: RandomizableConstraintSystem,
    {
        // Build a sponge hasher
        let hasher_params = PoseidonSpongeParameters::default();
        let mut hasher = PoseidonHashGadget::new(hasher_params);
        hasher.batch_absorb(cs, values)?;

        hasher.squeeze(cs)
    }

    /// Hash two internal nodes in the (binary) Merkle tree, giving the tree value at
    /// the parent node
    fn hash_internal_nodes<CS: RandomizableConstraintSystem>(
        left: &LinearCombination,
        right: &LinearCombination,
        cs: &mut CS,
    ) -> Result<LinearCombination, R1CSError> {
        let hasher_params = PoseidonSpongeParameters::default();
        let mut hasher = PoseidonHashGadget::new(hasher_params);
        hasher.batch_absorb(cs, &[left.clone(), right.clone()])?;

        hasher.squeeze(cs)
    }
}

/// The witness to the statement defined by the Merkle gadget; that is one of
/// Merkle inclusion
#[derive(Clone, Debug)]
pub struct MerkleWitness {
    /// The opening from the leaf node to the root, i.e. the set of sister nodes
    /// that hash together with the input from the leaf to the root
    pub opening: Vec<Scalar>,
    /// The preimage for the leaf i.e. the value that is sponge hashed into the leaf
    pub leaf_data: Vec<Scalar>,
}

/// The statement parameterization of the Merkle inclusion proof of knowledge
#[derive(Clone, Debug)]
pub struct MerkleStatement {
    /// The expected value of the root after hashing from the leaf
    pub expected_root: Scalar,
    /// The tree height, used to partition the commitments when given
    /// to the verifier as a vector
    pub tree_height: usize,
}

impl SingleProverCircuit for PoseidonMerkleHashGadget {
    type Statement = MerkleStatement;
    type Witness = MerkleWitness;

    const BP_GENS_CAPACITY: usize = 4096;

    fn prove(
        witness: Self::Witness,
        statement: Self::Statement,
        mut prover: Prover,
    ) -> Result<(Vec<CompressedRistretto>, R1CSProof), R1CSError> {
        // Commit to the witness
        let mut rng = OsRng {};
        let leaf_data_length = witness.leaf_data.len();
        let opening_len = witness.opening.len();

        let (leaf_comm, leaf_vars): (Vec<CompressedRistretto>, Vec<Variable>) = witness
            .leaf_data
            .into_iter()
            .zip((0..leaf_data_length).map(|_| Scalar::random(&mut rng)))
            .map(|(val, blinder)| prover.commit(val, blinder))
            .unzip();
        let (opening_comm, opening_vars): (Vec<CompressedRistretto>, Vec<Variable>) = witness
            .opening
            .into_iter()
            .zip((0..opening_len).map(|_| Scalar::random(&mut rng)))
            .map(|(val, blind)| prover.commit(val, blind))
            .unzip();

        // Commit to the expected root
        let (_, root_var) = prover.commit_public(statement.expected_root);

        // Apply the constraints
        PoseidonMerkleHashGadget::compute_and_constrain_root(
            &mut prover,
            leaf_vars,
            opening_vars,
            root_var,
        )?;

        // Prove the statement
        let bp_gens = BulletproofGens::new(Self::BP_GENS_CAPACITY, 1 /* party_capacity */);
        let proof = prover.prove(&bp_gens)?;

        Ok((
            opening_comm
                .into_iter()
                .chain(leaf_comm.into_iter())
                .collect_vec(),
            proof,
        ))
    }

    fn verify(
        witness_commitments: &[CompressedRistretto],
        statement: Self::Statement,
        proof: R1CSProof,
        mut verifier: Verifier,
    ) -> Result<(), R1CSError> {
        // Commit to the witness
        let leaf_vars = witness_commitments[statement.tree_height - 1..]
            .iter()
            .map(|comm| verifier.commit(*comm))
            .collect_vec();
        let opening_vars = witness_commitments[..statement.tree_height - 1]
            .iter()
            .map(|comm| verifier.commit(*comm))
            .collect_vec();

        let root_var = verifier.commit_public(statement.expected_root);

        // Apply constraints
        PoseidonMerkleHashGadget::compute_and_constrain_root(
            &mut verifier,
            leaf_vars,
            opening_vars,
            root_var,
        )?;

        // Verify the proof
        let bp_gens = BulletproofGens::new(Self::BP_GENS_CAPACITY, 1 /* party_capacity */);
        verifier.verify(&proof, &bp_gens)
    }
}

#[cfg(test)]
mod merkle_test {
    use ark_crypto_primitives::{
        crh::poseidon::{TwoToOneCRH, CRH},
        merkle_tree::{Config, IdentityDigestConverter},
        MerkleTree,
    };
    use curve25519_dalek::scalar::Scalar;
    use itertools::Itertools;
    use mpc_bulletproof::r1cs_mpc::R1CSError;
    use rand_core::OsRng;

    use crate::{
        mpc_gadgets::poseidon::PoseidonSpongeParameters,
        test_helpers::{
            bulletproof_prove_and_verify, convert_params, felt_to_scalar, scalar_to_prime_field,
            TestField,
        },
        zk_gadgets::merkle::PoseidonMerkleHashGadget,
    };

    use super::{MerkleStatement, MerkleWitness};

    struct MerkleConfig {}
    impl Config for MerkleConfig {
        type Leaf = [TestField];
        type LeafDigest = TestField;
        type InnerDigest = TestField;

        type LeafHash = CRH<TestField>;
        type TwoToOneHash = TwoToOneCRH<TestField>;
        type LeafInnerDigestConverter = IdentityDigestConverter<TestField>;
    }

    #[test]
    fn test_against_arkworks() {
        // A random input at the leaf
        let mut rng = OsRng {};
        let n = 6;
        let tree_height = 10;
        let leaf_data = (0..n).map(|_| Scalar::random(&mut rng)).collect_vec();

        // Compute the correct root via Arkworks
        let poseidon_config = PoseidonSpongeParameters::default();
        let arkworks_params = convert_params(&poseidon_config);

        let arkworks_leaf_data = leaf_data.iter().map(scalar_to_prime_field).collect_vec();

        let mut merkle_tree =
            MerkleTree::<MerkleConfig>::blank(&arkworks_params, &arkworks_params, tree_height)
                .unwrap();

        merkle_tree
            .update(0 /* index */, &arkworks_leaf_data)
            .unwrap();

        let expected_root = felt_to_scalar(&merkle_tree.root());
        let opening = merkle_tree.generate_proof(0 /* index */).unwrap();
        let mut opening_scalars = opening
            .auth_path
            .iter()
            .rev() // Path comes in reverse
            .map(felt_to_scalar)
            .collect_vec();

        // Add a zero to the opening scalar for the next leaf
        opening_scalars.insert(0, Scalar::zero());

        // Prove and verify the statement
        let witness = MerkleWitness {
            leaf_data,
            opening: opening_scalars,
        };

        let statement = MerkleStatement {
            expected_root,
            tree_height,
        };

        bulletproof_prove_and_verify::<PoseidonMerkleHashGadget>(witness, statement).unwrap();
    }

    #[test]
    fn test_invalid_witness() {
        // A random input at the leaf
        let mut rng = OsRng {};
        let n = 6;
        let tree_height = 10;
        let leaf_data = (0..n).map(|_| Scalar::random(&mut rng)).collect_vec();

        // Compute the correct root via Arkworks
        let poseidon_config = PoseidonSpongeParameters::default();
        let arkworks_params = convert_params(&poseidon_config);

        let arkworks_leaf_data = leaf_data.iter().map(scalar_to_prime_field).collect_vec();

        let mut merkle_tree =
            MerkleTree::<MerkleConfig>::blank(&arkworks_params, &arkworks_params, tree_height)
                .unwrap();

        merkle_tree
            .update(0 /* index */, &arkworks_leaf_data)
            .unwrap();

        // Random (incorrect) root
        let expected_root = Scalar::random(&mut rng);
        let opening = merkle_tree.generate_proof(0 /* index */).unwrap();
        let mut opening_scalars = opening
            .auth_path
            .iter()
            .rev() // Path comes in reverse
            .map(felt_to_scalar)
            .collect_vec();

        // Add a zero to the opening scalar for the next leaf
        opening_scalars.insert(0, Scalar::zero());

        // Prove and verify the statement
        let witness = MerkleWitness {
            leaf_data,
            opening: opening_scalars,
        };

        let statement = MerkleStatement {
            expected_root,
            tree_height,
        };

        let res = bulletproof_prove_and_verify::<PoseidonMerkleHashGadget>(witness, statement);
        assert_eq!(res, Err(R1CSError::VerificationError));
    }
}