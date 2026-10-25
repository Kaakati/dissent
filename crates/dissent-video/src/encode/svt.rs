//! AV1 encoder on SVT-AV1, through `shiguredo_svt_av1`, which builds SVT-AV1 from source.
//!
//! Real-time coding at preset 12 is the measured choice for calls. The "Video" section of
//! `docs/design/04-calls.md` has the timings.

use std::num::NonZeroUsize;

use shiguredo_svt_av1::{
    ColorFormat, EncodeOptions, Encoder, EncoderConfig as SvtConfig, FrameData, RcMode,
};

use super::{check_frame, validate, EncodedChunk, EncoderConfig, PtsQueue, VideoEncoder};
use crate::error::{Result, VideoError};
use crate::frame::Frame;

/// SVT-AV1 preset. 0 is the best quality and 13 the fastest. 12 is real-time.
pub const ENC_MODE: u8 = 12;

pub struct SvtEncoder {
    config: EncoderConfig,
    encoder: Encoder,
    keyframe_pending: bool,
    pts: PtsQueue,
    /// Chunks flushed from an old encoder during a rate change, returned on the next call.
    pending: Vec<EncodedChunk>,
}

impl SvtEncoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        validate(&config)?;
        Ok(Self {
            encoder: build(&config)?,
            config,
            keyframe_pending: false,
            pts: PtsQueue::new(),
            pending: Vec::new(),
        })
    }

    /// Emits every frame the encoder has finished, in order.
    fn drain(encoder: &mut Encoder, pts: &mut PtsQueue) -> Result<Vec<EncodedChunk>> {
        let mut out = Vec::new();
        while let Some(frame) = encoder.next_frame().map_err(encode_error)? {
            let index = frame.pts().max(0) as u64;
            out.push(EncodedChunk {
                data: frame.data().to_vec(),
                keyframe: frame.is_keyframe(),
                pts_us: pts.take(index),
            });
        }
        Ok(out)
    }
}

impl VideoEncoder for SvtEncoder {
    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn encode(&mut self, frame: &Frame) -> Result<Vec<EncodedChunk>> {
        check_frame(frame, &self.config)?;
        let i420 = frame.to_i420();
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (y, rest) = i420.data().split_at(w * h);
        let (u, v) = rest.split_at(w * h / 4);

        let force_keyframe = std::mem::take(&mut self.keyframe_pending);
        self.pts.push(frame.pts_us());
        self.encoder
            .encode(
                &FrameData::I420 { y, u, v },
                &EncodeOptions { force_keyframe },
            )
            .map_err(encode_error)?;

        let mut out = std::mem::take(&mut self.pending);
        out.extend(Self::drain(&mut self.encoder, &mut self.pts)?);
        Ok(out)
    }

    fn request_keyframe(&mut self) {
        self.keyframe_pending = true;
    }

    fn set_bitrate(&mut self, kbps: u32) -> Result<()> {
        if kbps == 0 {
            return Err(VideoError::Encode("bitrate must be above zero".into()));
        }
        if kbps == self.config.bitrate_kbps {
            return Ok(());
        }
        let next = EncoderConfig {
            bitrate_kbps: kbps,
            ..self.config
        };
        // SVT-AV1 fixes its rate control at creation. Finish the old encoder so no
        // look-ahead frame is lost, and keep its chunks for the next call.
        self.encoder.finish().map_err(encode_error)?;
        let flushed = Self::drain(&mut self.encoder, &mut self.pts)?;
        self.pending.extend(flushed);
        self.encoder = build(&next)?;
        self.config = next;
        self.pts = PtsQueue::new();
        Ok(())
    }
}

fn build(config: &EncoderConfig) -> Result<Encoder> {
    let mut svt = SvtConfig::new(
        config.width as usize,
        config.height as usize,
        ColorFormat::I420,
    );
    svt.fps_numerator = config.fps as usize;
    svt.fps_denominator = 1;
    svt.rate_control_mode = RcMode::Cbr;
    // SVT-AV1 takes bits per second.
    svt.target_bit_rate = config.bitrate_kbps as usize * 1_000;
    svt.enc_mode = ENC_MODE;
    svt.rtc = Some(true);
    svt.intra_period_length = NonZeroUsize::new(config.keyframe_interval as usize);
    Encoder::new(svt).map_err(encode_error)
}

fn encode_error(error: impl std::fmt::Display) -> VideoError {
    VideoError::Encode(error.to_string())
}
