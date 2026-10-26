//! AV1 encoder on SVT-AV1, through `svt-av1-sys`, which builds SVT-AV1 from the pinned submodule.
//!
//! Real-time coding at preset 12 is the measured choice for calls. The "Video" section of
//! `docs/design/04-calls.md` has the timings.

use std::{mem, ptr, slice};

use svt_av1_sys as sys;

use super::{check_frame, validate, EncodedChunk, EncoderConfig, PtsQueue, VideoEncoder};
use crate::error::{Result, VideoError};
use crate::frame::Frame;

/// SVT-AV1 preset. 0 is the best quality and 13 the fastest. 12 is real-time.
pub const ENC_MODE: i8 = 12;

/// SVT reads `SVT_LOG` once, when its first encoder is created. Level 2 is error only, so the
/// per-encoder `Svt[info]` lines stay off the console. A value the caller already set wins.
fn quiet_svt_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("SVT_LOG").is_none() {
            std::env::set_var("SVT_LOG", "2");
        }
    });
}

pub struct SvtEncoder {
    encoder: Encoder,
    config: EncoderConfig,
    keyframe_pending: bool,
    /// Frames submitted to the current encoder. SVT hands each one back by this index.
    submitted: i64,
    pts: PtsQueue,
    pending: Vec<EncodedChunk>,
}

/// One SVT-AV1 encoder instance. Dropping it closes the handle.
struct Encoder {
    handle: *mut sys::EbComponentType,
    width: u32,
    /// The configuration SVT was initialised with. SVT keeps the address of this struct, so it
    /// must outlive the handle.
    config: Box<sys::EbSvtAv1EncConfiguration>,
    /// Set once end-of-stream has been sent, so a later drop does not send it again.
    ended: bool,
}

// SAFETY: the handle is owned by this value alone, and every call on it takes `&mut self`, so
// two threads never drive one SVT instance at once. SVT-AV1 has no thread-affine state.
unsafe impl Send for Encoder {}

/// A finished packet from the encoder, copied out of SVT's buffer.
struct Packet {
    data: Vec<u8>,
    keyframe: bool,
    pts: i64,
    end_of_stream: bool,
}

fn check(error: sys::EbErrorType) -> Result<()> {
    if error == sys::EbErrorType_EB_ErrorNone {
        Ok(())
    } else {
        Err(VideoError::Encode(format!("SVT-AV1 error {error:#x}")))
    }
}

impl Encoder {
    fn open(config: &EncoderConfig) -> Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: SVT writes a fresh handle into `handle` and fills the configuration with its
        // defaults. The configuration is boxed, because SVT keeps its address, so it moves into
        // the encoder below and lives as long as the handle does.
        let mut svt: Box<sys::EbSvtAv1EncConfiguration> = Box::new(unsafe { mem::zeroed() });
        check(unsafe { sys::svt_av1_enc_init_handle(&mut handle, &mut *svt) })?;
        let mut encoder = Self {
            handle,
            width: config.width,
            config: svt,
            ended: false,
        };

        let svt = &mut encoder.config;
        svt.source_width = config.width;
        svt.source_height = config.height;
        svt.encoder_color_format = sys::EbColorFormat_EB_YUV420;
        svt.frame_rate_numerator = config.fps;
        svt.frame_rate_denominator = 1;
        svt.enc_mode = ENC_MODE;
        svt.rate_control_mode = sys::SvtAv1RcMode_SVT_AV1_RC_MODE_CBR as _;
        // SVT-AV1 takes bits per second.
        svt.target_bit_rate = config.bitrate_kbps * 1_000;
        svt.pred_structure = sys::PredStructure_LOW_DELAY as _;
        svt.intra_period_length = config.keyframe_interval as i32;
        svt.rtc = true;
        svt.scene_change_detection = 0;

        // SAFETY: `handle` came from `svt_av1_enc_init_handle` above and is still open, and the
        // configuration it is given is the boxed one the encoder owns.
        check(unsafe { sys::svt_av1_enc_set_parameter(encoder.handle, &mut **svt) })?;
        // SAFETY: the parameters are set and the handle is still open.
        check(unsafe { sys::svt_av1_enc_init(encoder.handle) })?;
        Ok(encoder)
    }

    /// Queues one I420 picture. `keyframe` makes SVT code it as a key frame. As the SVT reference
    /// app does, the buffer points at an `EbSvtIOFormat` naming the three planes, and the filled
    /// length is the frame's byte count, which SVT checks against the configured size.
    fn send(&mut self, y: &[u8], u: &[u8], v: &[u8], keyframe: bool, pts: i64) -> Result<()> {
        let stride = self.width;
        let chroma_stride = self.width / 2;
        let mut planes = sys::EbSvtIOFormat {
            luma: y.as_ptr() as *mut u8,
            cb: u.as_ptr() as *mut u8,
            cr: v.as_ptr() as *mut u8,
            y_stride: stride,
            cr_stride: chroma_stride,
            cb_stride: chroma_stride,
        };
        let frame_bytes = (y.len() + u.len() + v.len()) as u32;
        // SAFETY: a zeroed buffer header is valid, and the fields SVT reads are set below.
        let mut header: sys::EbBufferHeaderType = unsafe { mem::zeroed() };
        header.size = mem::size_of::<sys::EbBufferHeaderType>() as u32;
        header.p_buffer = (&mut planes as *mut sys::EbSvtIOFormat).cast();
        header.n_filled_len = frame_bytes;
        header.n_alloc_len = frame_bytes;
        header.pts = pts;
        header.pic_type = if keyframe {
            sys::EbAv1PictureType_EB_AV1_KEY_PICTURE
        } else {
            sys::EbAv1PictureType_EB_AV1_INVALID_PICTURE
        };
        // SAFETY: the handle is open. SVT copies the planes out before this call returns, so the
        // borrowed slices and `planes` only need to live for the call.
        check(unsafe { sys::svt_av1_enc_send_picture(self.handle, &mut header) })
    }

    /// Sends the end-of-stream marker, so SVT flushes every frame it still holds.
    fn end(&mut self) -> Result<()> {
        // SAFETY: a zeroed header with the end-of-stream flag is the marker SVT expects.
        let mut header: sys::EbBufferHeaderType = unsafe { mem::zeroed() };
        header.size = mem::size_of::<sys::EbBufferHeaderType>() as u32;
        header.flags = sys::EB_BUFFERFLAG_EOS;
        self.ended = true;
        // SAFETY: the handle is open.
        check(unsafe { sys::svt_av1_enc_send_picture(self.handle, &mut header) })
    }

    /// The next finished packet, or `None` while SVT has none ready.
    fn next_packet(&mut self, pictures_done: bool) -> Result<Option<Packet>> {
        let mut out: *mut sys::EbBufferHeaderType = ptr::null_mut();
        // SAFETY: SVT stores a buffer it owns in `out`, or leaves it null. The buffer is released
        // with `svt_av1_enc_release_out_buffer` below, once its bytes are copied out.
        let error = unsafe {
            sys::svt_av1_enc_get_packet(self.handle, &mut out, u8::from(pictures_done))
        };
        if error == sys::EbErrorType_EB_NoErrorEmptyQueue {
            return Ok(None);
        }
        check(error)?;
        if out.is_null() {
            return Ok(None);
        }
        // SAFETY: `out` is non-null and SVT keeps it valid until it is released below.
        let header = unsafe { &*out };
        let data = if header.n_filled_len == 0 || header.p_buffer.is_null() {
            Vec::new()
        } else {
            // SAFETY: SVT sets `p_buffer` to `n_filled_len` readable bytes while the buffer lives.
            unsafe { slice::from_raw_parts(header.p_buffer, header.n_filled_len as usize) }.to_vec()
        };
        let packet = Packet {
            data,
            keyframe: header.pic_type == sys::EbAv1PictureType_EB_AV1_KEY_PICTURE,
            pts: header.pts,
            end_of_stream: header.flags & sys::EB_BUFFERFLAG_EOS != 0,
        };
        // SAFETY: `out` came from `svt_av1_enc_get_packet`, and its bytes were copied out above.
        unsafe { sys::svt_av1_enc_release_out_buffer(&mut out) };
        Ok(Some(packet))
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SVT logs an error when it is deinitialised before end-of-stream. Flush the frames it still
        // holds first, then release them. A stream that already ended has nothing left to flush.
        if !self.ended && self.end().is_ok() {
            while let Ok(Some(packet)) = self.next_packet(true) {
                if packet.end_of_stream {
                    break;
                }
            }
        }
        // SAFETY: the handle is open until this drop, and is closed exactly once here.
        unsafe {
            let _ = sys::svt_av1_enc_deinit(self.handle);
            let _ = sys::svt_av1_enc_deinit_handle(self.handle);
        }
    }
}

impl SvtEncoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        validate(&config)?;
        quiet_svt_log();
        Ok(Self {
            encoder: Encoder::open(&config)?,
            config,
            keyframe_pending: false,
            submitted: 0,
            pts: PtsQueue::new(),
            pending: Vec::new(),
        })
    }

    /// Takes the packets SVT has finished. In low-delay mode SVT returns one packet per picture,
    /// and its call blocks until that packet is ready, so this takes exactly one packet per picture
    /// and no more. After the end-of-stream marker it takes every packet until the stream ends.
    fn drain(encoder: &mut Encoder, pts: &mut PtsQueue, stream_ended: bool) -> Result<Vec<EncodedChunk>> {
        let mut out = Vec::new();
        while let Some(packet) = encoder.next_packet(stream_ended)? {
            if packet.end_of_stream {
                break;
            }
            let index = packet.pts.max(0) as u64;
            out.push(EncodedChunk {
                data: packet.data,
                keyframe: packet.keyframe,
                pts_us: pts.take(index),
            });
            if !stream_ended {
                break;
            }
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

        let keyframe = std::mem::take(&mut self.keyframe_pending);
        self.pts.push(frame.pts_us());
        self.encoder.send(y, u, v, keyframe, self.submitted)?;
        self.submitted += 1;

        let mut out = std::mem::take(&mut self.pending);
        out.extend(Self::drain(&mut self.encoder, &mut self.pts, false)?);
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
        // SVT-AV1 fixes its rate control at creation. Flush the old encoder so no look-ahead frame
        // is lost, and keep its packets for the next call.
        self.encoder.end()?;
        let flushed = Self::drain(&mut self.encoder, &mut self.pts, true)?;
        self.pending.extend(flushed);
        self.encoder = Encoder::open(&next)?;
        self.config = next;
        self.submitted = 0;
        self.pts = PtsQueue::new();
        Ok(())
    }
}
