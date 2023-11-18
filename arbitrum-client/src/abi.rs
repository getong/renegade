//! Solidity ABI definitions of smart contracts, events, and other on-chain
//! data structures used by the Arbitrum client.

use ethers::contract::abigen;

abigen!(
    DarkpoolContract,
    r#"[
        function isNullifierSpent(bytes memory nullifier) external view returns (bool)
        function getRoot() external view returns (bytes)
        function rootInHistory(bytes memory root) external view returns (bool)

        function newWallet(bytes memory wallet_blinder_share, bytes memory proof, bytes memory valid_wallet_create_statement_bytes) external
        function updateWallet(bytes memory wallet_blinder_share, bytes memory proof, bytes memory valid_wallet_update_statement_bytes, bytes memory public_inputs_signature) external
        function processMatchSettle(bytes memory party_0_match_payload, bytes memory party_0_valid_commitments_proof, bytes memory party_0_valid_reblind_proof, bytes memory party_1_match_payload, bytes memory party_1_valid_commitments_proof, bytes memory party_1_valid_reblind_proof, bytes memory valid_match_settle_proof, bytes memory valid_match_settle_statement_bytes,) external
    ]"#
);