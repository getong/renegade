//! Groups handlers for updating and managing order book state in response to
//! events elsewhere in the local node or the network

use circuit_types::wallet::Nullifier;
use circuits::{
    verify_singleprover_proof,
    zk_circuits::{
        commitment_links::verify_reblind_commitments_link,
        valid_commitments::SizedValidCommitments, valid_reblind::SizedValidReblind,
    },
};
use common::types::{
    gossip::{ClusterId, WrappedPeerId},
    network_order::{NetworkOrder, NetworkOrderState},
    proof_bundles::{OrderValidityProofBundle, OrderValidityWitnessBundle},
    wallet::OrderIdentifier,
};
use futures::executor::block_on;
use gossip_api::{
    cluster_management::{ClusterManagementMessage, ValidityWitnessRequest},
    gossip::{
        AuthenticatedGossipResponse, GossipOutbound, GossipRequest, GossipResponse, PubsubMessage,
    },
    orderbook_management::OrderInfoResponse,
};
use job_types::gossip_server::OrderBookManagementJob;
use libp2p::request_response::ResponseChannel;
use tracing::log;

use super::{errors::GossipError, server::GossipProtocolExecutor};

/// Error message emitted when an already-used nullifier is received
const ERR_NULLIFIER_USED: &str = "invalid nullifier, already used";
/// Error message emitted when two validity proofs are improperly commitment linked
const ERR_INVALID_PROOF_LINK: &str =
    "invalid proof link between VALID REBLIND and VALID COMMITMENTS";

impl GossipProtocolExecutor {
    /// Dispatches messages from the cluster regarding order book management
    pub(super) async fn handle_order_book_management_job(
        &self,
        message: OrderBookManagementJob,
    ) -> Result<(), GossipError> {
        match message {
            OrderBookManagementJob::OrderInfo {
                order_id,
                response_channel,
            } => {
                self.handle_order_info_request(order_id, response_channel)
                    .await
            }

            OrderBookManagementJob::OrderInfoResponse { info, .. } => {
                if let Some(order_info) = info {
                    self.handle_order_info_response(order_info).await?;
                }

                Ok(())
            }

            OrderBookManagementJob::OrderReceived {
                order_id,
                nullifier,
                cluster,
            } => self.handle_new_order(order_id, nullifier, cluster).await,

            OrderBookManagementJob::OrderProofUpdated {
                order_id,
                cluster,
                proof_bundle,
            } => {
                self.handle_new_validity_proof(order_id, cluster, proof_bundle)
                    .await
            }

            OrderBookManagementJob::OrderWitness {
                order_id,
                requesting_peer,
            } => {
                self.handle_validity_witness_request(order_id, requesting_peer)
                    .await
            }

            OrderBookManagementJob::OrderWitnessResponse { order_id, witness } => {
                self.handle_validity_witness_response(order_id, witness)
                    .await;
                Ok(())
            }
        }
    }

    /// Handles a request for order information from a peer
    async fn handle_order_info_request(
        &self,
        order_id: OrderIdentifier,
        response_channel: ResponseChannel<AuthenticatedGossipResponse>,
    ) -> Result<(), GossipError> {
        let order_info = self
            .global_state
            .read_order_book()
            .await
            .get_order_info(&order_id)
            .await;

        self.network_channel
            .send(GossipOutbound::Response {
                channel: response_channel,
                message: GossipResponse::OrderInfo(OrderInfoResponse {
                    order_id,
                    info: order_info,
                }),
            })
            .map_err(|err| GossipError::SendMessage(err.to_string()))?;

        Ok(())
    }

    /// Handles a response to a request for order info
    async fn handle_order_info_response(
        &self,
        mut order_info: NetworkOrder,
    ) -> Result<(), GossipError> {
        // Move fields out of `order_info` before transferring ownership
        let order_id = order_info.id;
        let proof = order_info.validity_proofs.take();

        // Index the order in the `Received` state
        let is_local = order_info.cluster == self.global_state.local_cluster_id;
        order_info.state = NetworkOrderState::Received;
        order_info.local = is_local;
        self.global_state.add_order(order_info).await;

        // If there is a proof attached to the order, verify it and transition to `Verified`
        if let Some(proof_bundle) = proof {
            // We can trust local (i.e. originating from cluster peers) proofs
            if !is_local {
                let self_clone = self.clone();
                let bundle_clone = proof_bundle.clone();

                tokio::task::spawn_blocking(move || {
                    block_on(self_clone.verify_validity_proofs(&bundle_clone))
                })
                .await
                .unwrap()?;
            }

            // If the order is a locally managed order, the local peer also needs a copy of the witness
            // so that it may link commitments between the validity proof and subsequent match/encryption
            // proofs
            if is_local {
                self.request_order_witness(order_id)?;
            }

            // Update the state of the order to `Verified` by attaching the verified validity proof
            self.global_state
                .add_order_validity_proofs(&order_id, proof_bundle)
                .await;
        }

        Ok(())
    }

    /// Handles a newly discovered order added to the book
    async fn handle_new_order(
        &self,
        order_id: OrderIdentifier,
        nullifier: Nullifier,
        cluster: ClusterId,
    ) -> Result<(), GossipError> {
        // Ensure that the nullifier has not been used for this order
        if !self
            .starknet_client()
            .check_nullifier_unused(nullifier)
            .await
            .map_err(|err| GossipError::StarknetRequest(err.to_string()))?
        {
            log::info!("received order with spent nullifier, skipping...");
            return Ok(());
        }

        let is_local = cluster == self.global_state.local_cluster_id;
        self.global_state
            .add_order(NetworkOrder::new(order_id, nullifier, cluster, is_local))
            .await;
        Ok(())
    }

    /// Handles a new validity proof attached to an order
    ///
    /// TODO: We also need to sanity check the statement variables with the contract state,
    /// e.g. merkle root, nullifiers, etc.
    async fn handle_new_validity_proof(
        &self,
        order_id: OrderIdentifier,
        cluster: ClusterId,
        proof_bundle: OrderValidityProofBundle,
    ) -> Result<(), GossipError> {
        let is_local = cluster.eq(&self.global_state.local_cluster_id);

        // Verify the proof
        if !is_local {
            let bundle_clone = proof_bundle.clone();
            let self_clone = self.clone();

            tokio::task::spawn_blocking(move || {
                block_on(self_clone.verify_validity_proofs(&bundle_clone))
            })
            .await
            .unwrap()?;
        }

        // Add the order to the book in the `Validated` state
        if !self
            .global_state
            .read_order_book()
            .await
            .contains_order(&order_id)
        {
            self.global_state
                .add_order(NetworkOrder::new(
                    order_id,
                    proof_bundle
                        .reblind_proof
                        .statement
                        .original_shares_nullifier,
                    cluster,
                    is_local,
                ))
                .await;
        }

        self.global_state
            .add_order_validity_proofs(&order_id, proof_bundle)
            .await;

        // If the order is locally managed, also fetch the wintess used in the proof,
        // this is used for proof linking. I.e. the local node needs the commitment parameters
        // for each witness element so that it may share commitments with future proofs
        if is_local {
            self.request_order_witness(order_id)?;
        }

        Ok(())
    }

    /// Requests a copy of the witness used in an order's validity proof for a locally
    /// managed order
    fn request_order_witness(&self, order_id: OrderIdentifier) -> Result<(), GossipError> {
        let message =
            ClusterManagementMessage::RequestOrderValidityWitness(ValidityWitnessRequest {
                order_id,
                sender: self.global_state.local_peer_id,
            });

        self.network_channel
            .send(GossipOutbound::Pubsub {
                topic: self.global_state.local_cluster_id.get_management_topic(),
                message: PubsubMessage::ClusterManagement {
                    cluster_id: self.global_state.local_cluster_id.clone(),
                    message,
                },
            })
            .map_err(|err| GossipError::SendMessage(err.to_string()))
    }

    /// Handles a request for a validity proof witness from a peer
    async fn handle_validity_witness_request(
        &self,
        order_id: OrderIdentifier,
        requesting_peer: WrappedPeerId,
    ) -> Result<(), GossipError> {
        // Sanity check that the requesting peer is part of the cluster,
        // authentication of the message is done at the network manager level,
        // so this check is a bit redundant, but worth doing
        {
            let info = self
                .global_state
                .read_peer_index()
                .await
                .get_peer_info(&requesting_peer)
                .await
                .ok_or_else(|| {
                    GossipError::MissingState("peer info not found in state".to_string())
                })?;

            if info.get_cluster_id() != self.global_state.local_cluster_id {
                return Ok(());
            }
        } // peer_index lock released

        // If the local peer has a copy of the witness stored locally, send it to the peer
        if let Some(order_info) = self
            .global_state
            .read_order_book()
            .await
            .get_order_info(&order_id)
            .await
        && let Some(witness) = order_info.validity_proof_witnesses
        {
            self.network_channel
                .send(GossipOutbound::Request { peer_id: requesting_peer, message: GossipRequest::ValidityWitness {
                    order_id, witness
                }})
                .map_err(|err| GossipError::SendMessage(err.to_string()))?;
        }

        Ok(())
    }

    /// Handle a response from a peer containing a witness for `VALID COMMITMENTS`
    async fn handle_validity_witness_response(
        &self,
        order_id: OrderIdentifier,
        witnesses: OrderValidityWitnessBundle,
    ) {
        self.global_state
            .read_order_book()
            .await
            .attach_validity_proof_witness(&order_id, witnesses)
            .await;
    }

    /// Verify the validity proofs (`VALID REBLIND` and `VALID COMMITMENTS`) of an incoming order
    ///
    /// Aside from proof verification, this involves validating the statement
    /// variables (e.g. merkle root) for the proof
    async fn verify_validity_proofs(
        &self,
        proof_bundle: &OrderValidityProofBundle,
    ) -> Result<(), GossipError> {
        // Clone the proof out from behind their references so that the verifier may
        // take ownership
        let reblind_proof = proof_bundle.copy_reblind_proof();
        let commitment_proof = proof_bundle.copy_commitment_proof();

        // Verify the two proofs are correctly commitment linked
        if !verify_reblind_commitments_link(&reblind_proof.commitment, &commitment_proof.commitment)
        {
            log::error!("received validity proof bundle with invalid proof linking");
            return Err(GossipError::ValidCommitmentVerification(
                ERR_INVALID_PROOF_LINK.to_string(),
            ));
        }

        // Check that the proof shares' nullifiers are unused
        self.assert_nullifier_unused(reblind_proof.statement.original_shares_nullifier)
            .await?;

        // Check that the Merkle root is a valid historical root
        if !self
            .starknet_client()
            .check_merkle_root_valid(reblind_proof.statement.merkle_root)
            .await
            .map_err(|err| GossipError::StarknetRequest(err.to_string()))?
        {
            log::info!("got order with invalid merkle root, skipping...");
            // TODO: Once the contract implements foreign field Merkle, error on this test
            // return Err(GossipError::ValidCommitmentVerification(
            //     "invalid merkle root, not in contract history".to_string(),
            // ));
        }

        // Verify the reblind proof
        if let Err(e) = verify_singleprover_proof::<SizedValidReblind>(
            reblind_proof.statement,
            reblind_proof.commitment,
            reblind_proof.proof,
        ) {
            log::error!("Invalid proof of `VALID REBLIND`");
            return Err(GossipError::ValidReblindVerification(e.to_string()));
        }

        // Validate the commitment proof
        if let Err(e) = verify_singleprover_proof::<SizedValidCommitments>(
            commitment_proof.statement,
            commitment_proof.commitment,
            commitment_proof.proof,
        ) {
            log::error!("Invalid proof of `VALID COMMITMENTS`");
            return Err(GossipError::ValidCommitmentVerification(e.to_string()));
        }

        Ok(())
    }

    /// Assert that a nullifier is unused in the contract, returns a GossipError if
    /// the nullifier has been used
    async fn assert_nullifier_unused(&self, nullifier: Nullifier) -> Result<(), GossipError> {
        self.starknet_client()
            .check_nullifier_unused(nullifier)
            .await
            .map(|res| {
                if !res {
                    Err(GossipError::NullifierUsed(ERR_NULLIFIER_USED.to_string()))
                } else {
                    Ok(())
                }
            })
            .map_err(|err| GossipError::StarknetRequest(err.to_string()))?
    }
}