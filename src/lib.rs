pub const ATRAC3_SAMPLES_PER_FRAME: usize = 1024;

#[doc(hidden)]
pub mod atrac3;
#[doc(hidden)]
pub mod codec;
#[doc(hidden)]
pub mod dsp;
pub mod encoder;
#[doc(hidden)]
pub mod eval;
#[doc(hidden)]
pub mod gha;
#[doc(hidden)]
pub mod huffman;
#[doc(hidden)]
pub mod psychoacoustic;
pub mod riff;

pub use encoder::{choose_atrac3_block_align, EncodeOptions, EncodeSummary, Encoder};
