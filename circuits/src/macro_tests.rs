//! Defines tests for macros in the `circuit_macros` crate. We do this so that we may define the
//! bulk of the traits, data structures, etc outside of the `circuit-macros` crate; as a proc-macro
//! crate cannot export non proc-macro items

#[allow(clippy::missing_docs_in_private_items)]
#[cfg(test)]
mod test {
    use circuit_macros::circuit_type;
    use curve25519_dalek::{
        constants::RISTRETTO_BASEPOINT_POINT, ristretto::CompressedRistretto, scalar::Scalar,
    };
    use integration_helpers::mpc_network::mocks::{MockMpcNet, PartyIDBeaverSource};
    use merlin::Transcript;
    use mpc_bulletproof::{
        r1cs::{ConstraintSystem, LinearCombination, Prover, Variable, Verifier},
        r1cs_mpc::MpcProver,
        PedersenGens,
    };
    use mpc_ristretto::{
        authenticated_ristretto::AuthenticatedCompressedRistretto,
        authenticated_scalar::AuthenticatedScalar, beaver::SharedValueSource, network::MpcNetwork,
    };
    use rand_core::{CryptoRng, OsRng, RngCore};
    use std::ops::Add;
    use std::{cell::RefCell, rc::Rc};

    use crate::{
        mpc::{MpcFabric, SharedFabric},
        traits::{
            BaseType, CircuitBaseType, CircuitCommitmentType, CircuitVarType,
            LinearCombinationLike, LinkableBaseType, LinkableType, MpcBaseType,
            MpcLinearCombinationLike, MpcType, MultiproverCircuitBaseType,
            MultiproverCircuitCommitmentType, MultiproverCircuitVariableType, SecretShareBaseType,
            SecretShareType, SecretShareVarType,
        },
        LinkableCommitment,
    };

    #[circuit_type(
        singleprover_circuit,
        mpc,
        multiprover_circuit,
        linkable,
        multiprover_linkable,
        secret_share
    )]
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct TestType {
        val: Scalar,
    }

    impl TestType {
        fn check_equal(&self, val: Scalar) -> bool {
            self.val.eq(&val)
        }
    }

    #[test]
    fn test_base_type_preserved() {
        // Test that the base type may still be constructed
        let a = TestType { val: Scalar::one() };
        assert!(a.check_equal(Scalar::one()))
    }

    #[test]
    fn test_base_type_implementation() {
        let a = TestType {
            val: Scalar::from(2u8),
        };
        let serialized = a.to_scalars();
        let deserialized = TestType::from_scalars(&mut serialized.into_iter());

        assert_eq!(a, deserialized)
    }

    #[test]
    fn test_circuit_base_type_implementation() {
        let a = TestType { val: Scalar::one() };

        let mut rng = OsRng {};
        let pedersen_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pedersen_gens, &mut transcript);

        // Verify that we can commit to the type as a witness or public
        let (_, comm) = a.commit_witness(&mut rng, &mut prover);
        a.commit_public(&mut prover);

        // Verify that the derived commitment type may be committed to in a verifier
        let mut transcript = Transcript::new(b"test");
        let mut verifier = Verifier::new(&pedersen_gens, &mut transcript);

        comm.commit_verifier(&mut verifier);
    }

    #[test]
    fn test_circuit_base_type_derived_types() {
        let callback = |_: TestTypeCommitment| {};
        let a = TestType { val: Scalar::one() };

        let mut rng = OsRng {};
        let pedersen_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pedersen_gens, &mut transcript);

        // Commit to the type and verify that the callback typechecks
        let (_, comm) = a.commit_witness(&mut rng, &mut prover);
        callback(comm);
    }

    #[tokio::test]
    async fn test_mpc_derived_type() {
        let handle = tokio::task::spawn_blocking(|| {
            // Setup a dummy value to allocate then open
            let dummy = TestType {
                val: Scalar::from(2u8),
            };

            // Mock an MPC network
            let dummy_network = Rc::new(RefCell::new(MockMpcNet::new()));
            let dummy_network_data = vec![Scalar::one(); 100];
            dummy_network
                .borrow_mut()
                .add_mock_scalars(dummy_network_data);
            let dummy_beaver_source = Rc::new(RefCell::new(PartyIDBeaverSource::new(
                0, /* party_id */
            )));

            let dummy_fabric = MpcFabric::new_with_network(
                0, /* party_id */
                dummy_network,
                dummy_beaver_source,
            );
            let shared_fabric = SharedFabric::new(dummy_fabric);

            // Allocate the dummy value in the network
            let allocated = dummy
                .allocate(1 /* owning_party */, shared_fabric.clone())
                .unwrap();

            // Open the allocated value back to its original
            allocated.open(shared_fabric).unwrap();
        });

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_multiprover_derived_types() {
        let handle = tokio::task::spawn_blocking(|| {
            // Setup a dummy value to allocate in the constraint system
            let dummy = TestType {
                val: Scalar::from(2u8),
            };

            // Mock an MPC network
            let dummy_network = Rc::new(RefCell::new(MockMpcNet::new()));
            dummy_network
                .borrow_mut()
                .add_mock_points(vec![RISTRETTO_BASEPOINT_POINT; 100]);
            dummy_network
                .borrow_mut()
                .add_mock_scalars(vec![Scalar::one(); 100]);

            let dummy_beaver_source = Rc::new(RefCell::new(PartyIDBeaverSource::new(
                0, /* party_id */
            )));

            let dummy_fabric = MpcFabric::new_with_network(
                0, /* party_id */
                dummy_network,
                dummy_beaver_source,
            );
            let dummy_fabric = Rc::new(RefCell::new(dummy_fabric));
            let shared_fabric = SharedFabric(dummy_fabric.clone());

            // Mock a shared prover
            let pc_gens = PedersenGens::default();
            let mut transcript = Transcript::new(b"test");
            let mut prover = MpcProver::new_with_fabric(dummy_fabric, &mut transcript, &pc_gens);

            // Make a commitment into the shared constraint system
            let mut rng = OsRng {};
            let dummy_allocated = dummy.allocate(0, shared_fabric).unwrap();
            let (_, shared_comm) = dummy_allocated
                .commit_shared(&mut rng, &mut prover)
                .unwrap();

            // Open the commitment to its base type
            shared_comm.open().unwrap();
        });

        // The test will fail because the dummy data does not represent actual valid secret shares or
        // commitments. This is okay for testing the macros all that matters is that the types check
        #[allow(unused_must_use)]
        handle.await.unwrap_err();
    }

    #[test]
    fn test_linkable_commitments() {
        // Allocate a linkable type twice in the constraint system, verify that
        // its commitment stays the same
        let linkable_type = LinkableTestType {
            val: LinkableCommitment::from(Scalar::one()),
        };

        let pc_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pc_gens, &mut transcript);

        let mut rng = OsRng {};
        let (_, comm1) = linkable_type.commit_witness(&mut rng, &mut prover);
        let (_, comm2) = linkable_type.commit_witness(&mut rng, &mut prover);

        assert_eq!(comm1.val, comm2.val);
    }

    #[test]
    fn test_secret_share_types() {
        // Build two secret shares
        let share1 = TestTypeShare { val: Scalar::one() };
        let share2 = TestTypeShare { val: Scalar::one() };

        let recovered = share1.clone() + share2;
        assert_eq!(recovered.val, Scalar::from(2u8));

        // Blind a secret share
        let blinded = share1.blind(Scalar::one());
        assert_eq!(blinded.val, Scalar::from(2u8));

        // Unblind a secret share
        let unblinded = blinded.unblind(Scalar::one());
        assert_eq!(unblinded.val, Scalar::one());
    }

    #[test]
    fn test_secret_share_vars() {
        // Build a secret share and allocate it
        let share = TestTypeShare { val: Scalar::one() };

        // Build a mock constraint system
        let pc_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pc_gens, &mut transcript);

        // Allocate the secret share in the constraint system
        let mut rng = OsRng {};
        let _ = share.commit_witness(&mut rng, &mut prover);
    }

    #[test]
    fn test_secret_share_linkable_commitments() {
        // Build a secret share, commit to it twice, and verify the commitments are equal
        let share = LinkableTestTypeShare {
            val: Scalar::one().into(),
        };

        // Build a mock constraint system
        let pc_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pc_gens, &mut transcript);

        let mut rng = OsRng {};
        let (_, comm1) = share.commit_witness(&mut rng, &mut prover);
        let (_, comm2) = share.commit_witness(&mut rng, &mut prover);

        assert_eq!(comm1.val, comm2.val);
    }

    #[test]
    fn test_secret_share_var_arithmetic() {
        // Build two secret shares, allocate them in the constraint system, then evaluate their sum
        let share1 = TestTypeShare { val: Scalar::one() };
        let share2 = TestTypeShare { val: Scalar::one() };

        // Build a mock prover
        let pc_gens = PedersenGens::default();
        let mut transcript = Transcript::new(b"test");
        let mut prover = Prover::new(&pc_gens, &mut transcript);

        let mut rng = OsRng {};
        let (var1, _) = share1.commit_witness(&mut rng, &mut prover);
        let (var2, _) = share2.commit_witness(&mut rng, &mut prover);

        let sum = var1.clone() + var2;
        assert_eq!(Scalar::from(2u8), prover.eval(&sum.val));

        // Test blind and unblind
        let blinded = var1.blind(Variable::One());
        assert_eq!(Scalar::from(2u8), prover.eval(&blinded.val));

        let unblinded = blinded.unblind(Variable::One());
        assert_eq!(Scalar::one(), prover.eval(&unblinded.val));
    }
}