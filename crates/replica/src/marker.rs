//! The Replica store marker (`lait/replica/1`).
//!
//! The first thing opened in a Replica store. It distinguishes — before any
//! other file is trusted — a foreign directory, an unsupported store version, a
//! corrupt marker, and a valid store, so recreation guidance is exact and never
//! deletes or overwrites automatically. Integrity of the referenced material and
//! the lock are separate, later checks; this is only the 4 KiB header.

use lait_kernel::ids::SpaceId;
use serde::{Deserialize, Serialize};

/// The store magic.
pub const STORE_MAGIC: &[u8] = b"lait/replica/1";
/// The current store version.
pub const STORE_VERSION: u8 = 1;
/// Maximum marker header size.
pub const MAX_MARKER: usize = 4 * 1024;
/// The fixed rendered-SpaceId length.
pub const SPACE_ID_LEN: usize = 29;

/// The store marker header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreMarkerV1 {
    /// The magic bytes; must equal [`STORE_MAGIC`].
    pub magic: Vec<u8>,
    pub version: u8,
    pub space: [u8; SPACE_ID_LEN],
    /// BLAKE3 over `magic || [version] || space`.
    pub checksum: [u8; 32],
}

/// How a marker failed to identify a valid store. The last two are surfaced by
/// higher store-open logic, not the marker decoder, but share this taxonomy so
/// callers render one consistent recreation message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerError {
    /// The bytes are not a Replica store marker at all (foreign directory).
    NotAReplicaStore,
    /// A Replica marker of an unsupported version.
    UnsupportedStoreVersion { found: u8 },
    /// The marker decoded but failed its checksum/shape.
    CorruptStoreMarker,
    /// The store's referenced material failed full integrity validation.
    ReplicaIntegrityFailure,
    /// The store is locked by a live Station.
    ReplicaLocked,
}

impl std::fmt::Display for MarkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MarkerError {}

fn checksum(space: &[u8; SPACE_ID_LEN]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(STORE_MAGIC);
    h.update(&[STORE_VERSION]);
    h.update(space);
    *h.finalize().as_bytes()
}

impl StoreMarkerV1 {
    /// Build a marker for a Space's store.
    pub fn new(space: &SpaceId) -> Option<Self> {
        let space = <[u8; SPACE_ID_LEN]>::try_from(space.as_str().as_bytes()).ok()?;
        Some(Self {
            magic: STORE_MAGIC.to_vec(),
            version: STORE_VERSION,
            space,
            checksum: checksum(&space),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard marker")
    }

    /// Classify raw marker bytes. Order matters: magic first (foreign vs ours),
    /// then version (ours but unsupported), then checksum (corrupt), so each
    /// failure gets its exact typed cause.
    pub fn classify(bytes: &[u8]) -> Result<Self, MarkerError> {
        if bytes.len() > MAX_MARKER {
            return Err(MarkerError::CorruptStoreMarker);
        }
        let marker: Self =
            postcard::from_bytes(bytes).map_err(|_| MarkerError::NotAReplicaStore)?;
        if marker.magic != STORE_MAGIC {
            return Err(MarkerError::NotAReplicaStore);
        }
        if marker.version != STORE_VERSION {
            return Err(MarkerError::UnsupportedStoreVersion {
                found: marker.version,
            });
        }
        if marker.checksum != checksum(&marker.space) {
            return Err(MarkerError::CorruptStoreMarker);
        }
        Ok(marker)
    }

    /// The Space this store holds, if the marker is valid.
    pub fn space(&self) -> Option<SpaceId> {
        std::str::from_utf8(&self.space)
            .ok()
            .and_then(SpaceId::parse)
    }
}
