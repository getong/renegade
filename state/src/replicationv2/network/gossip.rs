//! Gossip networking interface, acts as a shim between raft and our gossip
//! layer

use async_trait::async_trait;
use gossip_api::request_response::{GossipRequest, GossipResponse};
use job_types::network_manager::{NetworkManagerJob, NetworkManagerQueue};
use openraft::error::{NetworkError, RPCError, RaftError};
use util::err_str;

use crate::replicationv2::{
    error::{new_network_error, ReplicationV2Error},
    Node, NodeId,
};

use super::{P2PNetworkFactory, P2PRaftNetwork, P2PRaftNetworkWrapper, RaftRequest, RaftResponse};

/// The error message emitted when a response type is invalid
const ERR_INVALID_RESPONSE: &str = "invalid response type from raft peer";

/// The network shim
#[derive(Clone)]
pub struct GossipNetwork {
    /// The target node for this instance
    target: NodeId,
    /// The target node info
    target_info: Node,
    /// A sender to the network manager's queue
    network_sender: NetworkManagerQueue,
}

impl GossipNetwork {
    /// Constructor
    pub fn new(target: NodeId, target_info: Node, network_sender: NetworkManagerQueue) -> Self {
        Self { target, target_info, network_sender }
    }

    /// Construct a new `GossipNetwork` instance without target specified
    pub fn empty(network_sender: NetworkManagerQueue) -> Self {
        Self { target: NodeId::default(), target_info: Node::default(), network_sender }
    }

    /// Convert a gossip response into a raft response
    fn to_raft_response(resp: GossipResponse) -> Result<RaftResponse, ReplicationV2Error> {
        let resp_bytes = match resp {
            GossipResponse::Raft(x) => x,
            _ => {
                return Err(ReplicationV2Error::Deserialize(ERR_INVALID_RESPONSE.to_string()));
            },
        };

        let raft_resp =
            bincode::deserialize(&resp_bytes).map_err(err_str!(ReplicationV2Error::Deserialize))?;
        Ok(raft_resp)
    }
}

#[async_trait]
impl P2PRaftNetwork for GossipNetwork {
    fn target(&self) -> NodeId {
        self.target
    }

    async fn send_request(
        &self,
        _target: NodeId,
        request: RaftRequest,
    ) -> Result<RaftResponse, RPCError<NodeId, Node, RaftError<NodeId>>> {
        // We serialize in the raft layer to avoid the `gossip-api` depending on `state`
        let ser =
            bincode::serialize(&request).map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        let req = GossipRequest::Raft(ser);

        // Send a network manager job
        let peer_id = self.target_info.peer_id;
        let (job, rx) = NetworkManagerJob::request_with_response(peer_id, req);
        self.network_sender.send(job).unwrap();

        // TODO: timeout and error handling
        let resp = rx.await.unwrap();
        Self::to_raft_response(resp).map_err(new_network_error)
    }
}

impl P2PNetworkFactory for GossipNetwork {
    fn new_p2p_client(&self, target: NodeId, target_info: Node) -> P2PRaftNetworkWrapper {
        let mut clone = self.clone();
        clone.target = target;
        clone.target_info = target_info;

        P2PRaftNetworkWrapper::new(clone)
    }
}