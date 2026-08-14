// Raw flatc output, one module per schema file (schema/*.fbs) -- generated code cross-references
// siblings via hardcoded `crate::<name>_generated::*` paths, so these must live at the crate
// root. `assignment_fb` re-exports all of them into one flat namespace for internal use.
#[allow(
    dead_code,
    unused_imports,
    unsafe_op_in_unsafe_fn,
    mismatched_lifetime_syntaxes,
    clippy::all
)]
mod assignment_generated {
    include!("../schema/gen/assignment_generated.rs");
}
#[allow(
    dead_code,
    unused_imports,
    unsafe_op_in_unsafe_fn,
    mismatched_lifetime_syntaxes,
    clippy::all
)]
mod worker_assignment_generated {
    include!("../schema/gen/worker_assignment_generated.rs");
}
#[allow(
    dead_code,
    unused_imports,
    unsafe_op_in_unsafe_fn,
    mismatched_lifetime_syntaxes,
    clippy::all
)]
mod portal_assignment_generated {
    include!("../schema/gen/portal_assignment_generated.rs");
}

/// The flatc-generated views the readers hand back: a chunk, a dataset, a worker entry. Public so
/// a consumer can name what `find_chunk`/`get_dataset`/`iter_chunks` return — reading a field off
/// one works without this, but writing a function or struct over it does not.
pub mod fb {
    pub use crate::{
        assignment_generated::{
            Chunk, ChunkHash, Dataset, EncryptedHeaders, FileUrl, TopRun, WorkerEntry, WorkerId,
        },
        portal_assignment_generated::{PortalAssignmentDataset, PortalEntry},
        worker_assignment_generated::{GenerationEntry, TableRoster, WorkerAssignmentDataset},
    };
}

mod assignment_fb;
#[cfg(feature = "builder")]
mod builder;
mod common;
#[cfg(feature = "reader")]
mod reader;
#[cfg(feature = "builder")]
mod signatures;

pub use common::{NetworkAssignment, NetworkState, SchemaBundle, WorkerStatus};

#[cfg(feature = "builder")]
pub use builder::AssignmentBuilder;
#[cfg(feature = "builder")]
pub use builder::{
    PortalAssignmentBuilder, PortalAssignmentChunkBuilder, WorkerAssignmentBuilder,
    WorkerAssignmentChunkBuilder,
};

#[cfg(feature = "reader")]
pub use reader::{
    AssignedWorker, InvalidAssignment, PortalAssignment, PortalChunk, PortalWorker,
    WorkerAssignment, WorkerChunk,
};
#[cfg(feature = "reader")]
pub use reader::{Assignment, ChunkNotFound, ChunkRef, Worker};
