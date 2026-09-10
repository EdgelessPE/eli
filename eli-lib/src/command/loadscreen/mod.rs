#[cfg(feature = "loadscreen-bake")]
mod bake;
mod play;

#[cfg(feature = "loadscreen-bake")]
pub use bake::{
    BakeEvent, BakeJobPhase, BakePreparationStage, BakeResult, DEFAULT_QUALITY, DEFAULT_SLICES,
    MAX_OUTPUT_EDGE, MAX_QUALITY, MAX_SLICES, bake,
};
pub use play::play_demo;
