use libp2p::{Multiaddr, PeerId};
use serde::{
    de::{Error as SerdeErr, Visitor},
    Deserialize, Serialize,
};
use std::{
    fmt::Display,
    ops::Deref,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

// Contains information about connected peers
#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    // The identifier used by libp2p for a peer
    peer_id: WrappedPeerId,

    // The multiaddr of the peer
    addr: Multiaddr,

    // Last time a successful hearbeat was received from this peer
    #[serde(skip)]
    last_heartbeat: AtomicU64,
}

impl PeerInfo {
    pub fn new(peer_id: WrappedPeerId, addr: Multiaddr) -> Self {
        Self {
            addr,
            peer_id,
            last_heartbeat: AtomicU64::new(current_time_seconds()),
        }
    }

    // Getters and Setters
    pub fn get_peer_id(&self) -> WrappedPeerId {
        self.peer_id
    }

    pub fn get_addr(&self) -> Multiaddr {
        self.addr.clone()
    }

    // Records a successful heartbeat
    pub fn successful_heartbeat(&mut self) {
        self.last_heartbeat
            .store(current_time_seconds(), Ordering::Relaxed);
    }

    pub fn get_last_heartbeat(&self) -> u64 {
        self.last_heartbeat.load(Ordering::Relaxed)
    }
}

// Clones PeerInfo to reference the curren time for the last heartbeat
impl Clone for PeerInfo {
    fn clone(&self) -> Self {
        Self {
            peer_id: self.peer_id,
            addr: self.addr.clone(),
            last_heartbeat: AtomicU64::new(self.last_heartbeat.load(Ordering::Relaxed)),
        }
    }
}

/**
 * An implementation of a wrapper type that allows us to implement traits
 * on top of the existing libp2p PeerID type
 */

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
// Wraps PeerID so that we can implement various traits on the type
pub struct WrappedPeerId(pub PeerId);

// Deref so that the wrapped type can be referenced
impl Deref for WrappedPeerId {
    type Target = PeerId;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for WrappedPeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// Serialize PeerIDs
impl Serialize for WrappedPeerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let bytes = self.to_bytes();
        serializer.serialize_bytes(&bytes)
    }
}

// Deserialize PeerIDs
impl<'de> Deserialize<'de> for WrappedPeerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(PeerIDVisitor)
    }
}

// Visitor struct for help deserializing PeerIDs
struct PeerIDVisitor;
impl<'de> Visitor<'de> for PeerIDVisitor {
    type Value = WrappedPeerId;

    // Debug message
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a libp2p::PeerID encoded as a byte array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut bytes_vec = Vec::new();
        while let Some(value) = seq.next_element()? {
            bytes_vec.push(value);
        }

        if let Ok(peer_id) = PeerId::from_bytes(&bytes_vec[..]) {
            return Ok(WrappedPeerId(peer_id));
        }

        Err(SerdeErr::custom("deserializing byte array to PeerID"))
    }
}

/**
 * Helpers
 */

// Returns a u64 representing the current unix timestamp in seconds
fn current_time_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("negative timestamp")
        .as_secs()
}