use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum WorkerStatus {
    Ok,
    Unreliable,
    DeprecatedVersion,
    UnsupportedVersion,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkAssignment {
    /// Deprecated: use `fb_url` or `fb_url_v1` instead.
    #[deprecated(note = "use fb_url or fb_url_v1 instead")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub fb_url: Option<String>,
    pub fb_url_v1: Option<String>,
    pub id: String,
    pub effective_from: u64,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkState {
    pub network: String,
    pub assignment: NetworkAssignment,

    #[cfg(feature = "mvcc-chunks")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_assignment: Option<NetworkAssignment>,

    #[cfg(feature = "mvcc-chunks")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_assignment: Option<NetworkAssignment>,
}
