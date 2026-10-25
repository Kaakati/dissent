//! H.264 encoder on OpenH264, the fallback for peers without AV1.
//!
//! OpenH264 emits one access unit per input frame and has no look-ahead, so each call
//! returns at most one chunk, stamped with the input frame's time.

use openh264::encoder::{
    BitRate, Encoder, EncoderConfig as OpenH264Config, FrameRate, FrameType, RateControlMode,
    UsageType,
};
use openh264::formats::YUVSlices;
use openh264::OpenH264API;

use super::{check_frame, validate, EncodedChunk, EncoderConfig, VideoEncoder};
use crate::error::{Result, VideoError};
use crate::frame::Frame;

pub struct H264Encoder {
    config: EncoderConfig,
    encoder: Encoder,
    keyframe_pending: bool,
}

impl H264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        validate(&config)?;
        Ok(Self {
            encoder: build(&config)?,
            config,
            keyframe_pending: false,
        })
    }
}

impl VideoEncoder for H264Encoder {
    fn config(&self) -> EncoderConfig {
        self.config
    }

    fn encode(&mut self, frame: &Frame) -> Result<Vec<EncodedChunk>> {
        check_frame(frame, &self.config)?;
        let i420 = frame.to_i420();
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (y, rest) = i420.data().split_at(w * h);
        let (u, v) = rest.split_at(w * h / 4);
        let yuv = YUVSlices::new((y, u, v), (w, h), (w, w / 2, w / 2));

        if std::mem::take(&mut self.keyframe_pending) {
            self.encoder.force_intra_frame();
        }
        let stream = self.encoder.encode(&yuv).map_err(encode_error)?;
        if stream.frame_type() == FrameType::Skip {
            return Ok(Vec::new());
        }
        let keyframe = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        Ok(vec![EncodedChunk {
            data: stream.to_vec(),
            keyframe,
            pts_us: frame.pts_us(),
        }])
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
        // OpenH264 has no live rate setter here, so rebuild. The rebuilt encoder starts on a keyframe.
        self.encoder = build(&next)?;
        self.config = next;
        Ok(())
    }
}

fn build(config: &EncoderConfig) -> Result<Encoder> {
    let openh264 = OpenH264Config::new()
        .bitrate(BitRate::from_bps(config.bitrate_kbps * 1_000))
        .max_frame_rate(FrameRate::from_hz(config.fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        .usage_type(UsageType::CameraVideoRealTime);
    Encoder::with_api_config(OpenH264API::from_source(), openh264).map_err(encode_error)
}

fn encode_error(error: impl std::fmt::Display) -> VideoError {
    VideoError::Encode(error.to_string())
}
