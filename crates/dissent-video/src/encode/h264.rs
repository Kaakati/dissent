//! H.264 encoder on OpenH264, the fallback for peers without AV1.
//!
//! OpenH264 emits one access unit per input frame and has no look-ahead, so each call
//! returns at most one chunk, stamped with the input frame's time.

use std::os::raw::c_int;

use openh264::encoder::{
    BitRate, Encoder, EncoderConfig as OpenH264Config, FrameRate, FrameType, IntraFramePeriod, RateControlMode,
    UsageType,
};
use openh264::formats::YUVSlices;
use openh264::OpenH264API;
use openh264_sys2::{SBitrateInfo, ENCODER_OPTION, ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, LAYER_NUM, SPATIAL_LAYER_0, SPATIAL_LAYER_ALL};

use super::{check_frame, validate, EncodedChunk, EncoderConfig, VideoEncoder};
use crate::error::{Result, VideoError};
use crate::frame::Frame;

pub struct H264Encoder {
    config: EncoderConfig,
    encoder: Encoder,
    keyframe_pending: bool,
    /// Whether a frame went through. OpenH264 sets itself up on the first frame, and until then it
    /// has no rate to change.
    started: bool,
    /// The ceiling OpenH264 was given for the rate, in kbps.
    max_kbps: u32,
}

impl H264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        validate(&config)?;
        Ok(Self {
            encoder: build(&config)?,
            config,
            keyframe_pending: false,
            started: false,
            max_kbps: 0,
        })
    }
}

impl H264Encoder {
    fn set_rate(&mut self, option: ENCODER_OPTION, layer: LAYER_NUM, kbps: u32) -> Result<()> {
        let mut info = SBitrateInfo {
            iLayer: layer,
            iBitrate: (kbps * 1_000) as c_int,
        };
        // SAFETY: the encoder is set up, since a frame went through it, and OpenH264 copies the rate
        // out of `info` before this call returns.
        let status = unsafe { self.encoder.raw_api().set_option(option, (&raw mut info).cast()) };
        if status != 0 {
            return Err(VideoError::Encode(format!("OpenH264 refused the bitrate: {status}")));
        }
        Ok(())
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
        self.started = true;
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
        if self.started {
            // OpenH264 refuses a target above the layer's ceiling, so a rise sets a ceiling at twice the
            // rate first. Equal, the two would hold the rate below target. A fall leaves the ceiling.
            if kbps > self.config.bitrate_kbps && kbps * 2 > self.max_kbps {
                self.set_rate(ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_0, kbps * 2)?;
                self.max_kbps = kbps * 2;
            }
            self.set_rate(ENCODER_OPTION_BITRATE, SPATIAL_LAYER_ALL, kbps)?;
        } else {
            self.encoder = build(&next)?;
        }
        self.config = next;
        Ok(())
    }
}

fn build(config: &EncoderConfig) -> Result<Encoder> {
    let openh264 = OpenH264Config::new()
        .bitrate(BitRate::from_bps(config.bitrate_kbps * 1_000))
        .max_frame_rate(FrameRate::from_hz(config.fps as f32))
        .rate_control_mode(RateControlMode::Bitrate)
        // A keyframe on a clock, as a safety net: one a receiver asked for and lost would otherwise never come.
        .intra_frame_period(IntraFramePeriod::from_num_frames(config.keyframe_interval))
        // Screen-content mode turns this off itself, with a warning on the console.
        .adaptive_quantization(!config.screen)
        .usage_type(if config.screen {
            UsageType::ScreenContentRealTime
        } else {
            UsageType::CameraVideoRealTime
        });
    Encoder::with_api_config(OpenH264API::from_source(), openh264).map_err(encode_error)
}

fn encode_error(error: impl std::fmt::Display) -> VideoError {
    VideoError::Encode(error.to_string())
}
