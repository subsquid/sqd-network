use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum WorkerStatus {
    Ok,
    Unreliable,
    DeprecatedVersion,
    UnsupportedVersion,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NetworkAssignment {
    /// Deprecated: use `fb_url_v1` instead.
    #[deprecated(note = "use fb_url_v1 instead")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Deprecated: use `fb_url_v1` instead.
    #[deprecated(note = "use fb_url_v1 instead")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fb_url: Option<String>,
    pub fb_url_v1: Option<String>,
    pub id: String,
    pub effective_from: u64,
}

/// Where to fetch the schema content that assignments only reference by id — a worker chunk's
/// `write_schema_id`, a portal dataset's `read_schema_id`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SchemaBundle {
    /// Content hash of the bundle at `url`, so a cached copy can be reused across states.
    pub hash: String,
    pub url: String,
}

/// The published network state.
///
/// Migrating to split assignments walks a state through three shapes: `assignment` alone, then
/// `assignment` alongside `worker_assignment`/`portal_assignment` while consumers switch over,
/// then the split pair alone. Every field but `network` is therefore optional — which shapes are
/// actually valid is the publisher's call, not this type's.
#[derive(Serialize, Deserialize, Debug)]
pub struct NetworkState {
    pub network: String,

    /// The combined assignment served identically to workers and portals. Optional: a network
    /// that has finished migrating stops publishing it, leaving the split blobs below as the
    /// only source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<NetworkAssignment>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_assignment: Option<NetworkAssignment>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_assignment: Option<NetworkAssignment>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_bundle: Option<SchemaBundle>,
}
