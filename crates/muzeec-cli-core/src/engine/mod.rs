//! Audio engine: decode, resample, mix, and output.

pub mod decode;
pub mod dsp;
pub mod gapless;
pub mod mixer;
pub mod output;
pub mod player;
pub mod resample;

pub use player::{
    Engine, EngineCommand, EngineConfig, EngineEvent, EngineHandle, PlaybackSnapshot,
    PlaybackState, PlaybackTrack,
};
