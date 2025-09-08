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
    pub url: String,
    pub fb_url: Option<String>,
    pub fb_url_v1: Option<String>,
    pub id: String,
    pub effective_from: u64,
}

#[derive(Serialize, Deserialize)]
pub struct NetworkState {
    pub network: String,
    pub assignment: NetworkAssignment,
}
