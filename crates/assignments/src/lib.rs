pub mod assignment_fb;
#[cfg(feature = "builder")]
mod builder;

#[cfg(feature = "builder")]
pub use builder::AssignmentBuilder;
