mod assignment_fb;
#[cfg(feature = "builder")]
mod builder;
mod common;
#[cfg(feature = "reader")]
mod reader;

pub use common::WorkerStatus;

#[cfg(feature = "builder")]
pub use builder::AssignmentBuilder;

#[cfg(feature = "reader")]
pub use reader::{Assignment, ChunkNotFound, Worker};
