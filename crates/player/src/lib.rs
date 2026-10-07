//! Audio engine: decoding, resampling and output.

mod decode;
mod resample;
mod stream;

pub use decode::{DecodeError, Decoder, Pcm};

pub use stream::ProgressiveFile;
pub use resample::Resampler;
