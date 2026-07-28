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
#[cfg(all(feature = "builder", feature = "mvcc-chunks"))]
pub use builder::{
    PortalAssignmentBuilder, PortalAssignmentChunkBuilder, WorkerAssignmentBuilder,
    WorkerAssignmentChunkBuilder,
};

#[cfg(all(feature = "reader", feature = "mvcc-chunks"))]
pub use reader::{AssignedWorker, PortalAssignment, PortalWorker, WorkerAssignment};
#[cfg(feature = "reader")]
pub use reader::{Assignment, ChunkNotFound, ChunkRef, Worker};
