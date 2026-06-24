//! QPlayer Audio Engine — real-time audio playback.
//!
//! Lock-free audio callbacks, sample-accurate fades, EQ, mixing.

pub mod biquad;
pub mod buffered_source;
pub mod channel_converter;
pub mod decoder;
pub mod engine;
pub mod eq_processor;
pub mod fade_processor;
pub mod limiter_processor;
pub mod loop_processor;
pub mod metering_processor;
pub mod mixer;
pub mod pan_processor;
pub mod resampler;
pub mod sample_provider;

pub use biquad::Biquad;
pub use buffered_source::BufferedSource;
pub use channel_converter::MonoToStereo;
pub use decoder::{DecodeError, FileDecoder};
pub use engine::{AudioEngine, AudioError};
pub use eq_processor::EqProcessor;
pub use fade_processor::FadeProcessor;
pub use limiter_processor::LimiterProcessor;
pub use loop_processor::LoopProcessor;
pub use metering_processor::{MeterData, MeteringProcessor};
pub use mixer::{db_to_linear, linear_to_db, Mixer, MixerInput, MIXER_CHANNELS, MIXER_SAMPLE_RATE};
pub use pan_processor::PanProcessor;
pub use resampler::ResamplerProcessor;
pub use sample_provider::{FnSource, SampleProvider};
