mod assignment_fb;
#[cfg(feature = "builder")]
mod builder;
mod common;
#[cfg(feature = "reader")]
mod reader;
#[cfg(feature = "builder")]
mod signatures;

pub use common::{NetworkAssignment, NetworkState, WorkerStatus};

#[cfg(feature = "builder")]
pub use builder::AssignmentBuilder;

#[cfg(feature = "reader")]
pub use reader::{Assignment, ChunkNotFound, Worker};
