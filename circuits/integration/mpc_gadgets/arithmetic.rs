//! Groups integration tests for arithmetic gadets used in the MPC circuits

use circuits::mpc_gadgets::arithmetic::{prefix_mul, product};
use integration_helpers::types::IntegrationTest;
use mpc_ristretto::{authenticated_scalar::AuthenticatedScalar, mpc_scalar::scalar_to_u64};
use rand::{thread_rng, Rng};

use crate::{IntegrationTestArgs, TestWrapper};

use super::{check_equal, check_equal_vec};

/// Tests the product gadget
fn test_product(test_args: &IntegrationTestArgs) -> Result<(), String> {
    // Each party decided on `n` values
    let n = 5;
    let mut rng = thread_rng();
    let my_values = (0..n)
        .map(|_| (rng.gen_range(0..100)) as u64)
        .collect::<Vec<u64>>();

    // Share the values
    let p1_values = test_args
        .borrow_fabric()
        .batch_allocate_private_u64s(0 /* owning_party */, &my_values)
        .map_err(|err| format!("Error sharing party 0's values: {:?}", err))?;

    let p2_values = test_args
        .borrow_fabric()
        .batch_allocate_private_u64s(1 /* owning_party */, &my_values)
        .map_err(|err| format!("Error allocating party 1's values: {:?}", err))?;

    let mut all_values = Vec::new();
    all_values.append(&mut p1_values.clone());
    all_values.append(&mut p2_values.clone());

    // Compute the product
    let res = product(&all_values, test_args.mpc_fabric.clone())
        .map_err(|err| format!("Error computing product: {:?}", err))?
        .open_and_authenticate()
        .map_err(|err| format!("Error opening and authenticating result: {:?}", err))?;

    // Open the shared values and compute the expected result
    let p1_values_prod = AuthenticatedScalar::batch_open(&p1_values)
        .map_err(|err| format!("Error opening p1_values: {:?}", err))?
        .iter()
        .fold(1u64, |acc, val| acc * scalar_to_u64(&val.to_scalar()));

    let p2_values_prod = AuthenticatedScalar::batch_open(&p2_values)
        .map_err(|err| format!("Error opening p2_values: {:?}", err))?
        .iter()
        .fold(1u64, |acc, val| acc * scalar_to_u64(&val.to_scalar()));

    let expected_result = p1_values_prod * p2_values_prod;
    check_equal(&res, expected_result)?;

    Ok(())
}

/// Tests the prefix-mul gadget
fn test_prefix_mul(test_args: &IntegrationTestArgs) -> Result<(), String> {
    // Compute powers of 3
    let n = 25;
    let value = 3;
    let shared_values = test_args
        .borrow_fabric()
        .batch_allocate_private_u64s(0 /* owning_party */, &vec![value; n])
        .map_err(|err| format!("Error allocating inputs: {:?}", err))?;

    // Run the prefix_mul gadget
    let prefixes = prefix_mul(&shared_values, test_args.mpc_fabric.clone())
        .map_err(|err| format!("Error computing prefix products: {:?}", err))?;

    // Open the prefixes and verify the result
    let opened_prefix_products = AuthenticatedScalar::batch_open_and_authenticate(&prefixes)
        .map_err(|err| format!("Error opening prefixes: {:?}", err))?;

    let mut expected_result = Vec::with_capacity(n);
    let mut acc = 1;
    for _ in 0..n {
        acc *= value;
        expected_result.push(acc);
    }

    check_equal_vec(&opened_prefix_products, &expected_result)?;
    Ok(())
}

// Take inventory
inventory::submit!(TestWrapper(IntegrationTest {
    name: "mpc_gadgets::arithmetic::test_product",
    test_fn: test_product
}));

inventory::submit!(TestWrapper(IntegrationTest {
    name: "mpc_gadgets::arithmetic::test_previx_mul",
    test_fn: test_prefix_mul
}));