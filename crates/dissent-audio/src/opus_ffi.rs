//! Raw declarations for the vendored libopus 1.5.2 (see `build.rs`). Only the calls the codec
//! wrapper uses are declared. Every call is `unsafe` and is wrapped in [`crate::codec`].

#![allow(non_camel_case_types, dead_code)]

use std::os::raw::{c_int, c_uchar};

/// Opaque encoder state.
#[repr(C)]
pub struct OpusEncoder {
    _private: [u8; 0],
}

/// Opaque decoder state.
#[repr(C)]
pub struct OpusDecoder {
    _private: [u8; 0],
}

pub const OPUS_OK: c_int = 0;
pub const OPUS_APPLICATION_VOIP: c_int = 2048;
pub const OPUS_SIGNAL_VOICE: c_int = 3001;

pub const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
pub const OPUS_SET_VBR_REQUEST: c_int = 4006;
pub const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;
pub const OPUS_SET_INBAND_FEC_REQUEST: c_int = 4012;
pub const OPUS_SET_PACKET_LOSS_PERC_REQUEST: c_int = 4014;
pub const OPUS_SET_DTX_REQUEST: c_int = 4016;
pub const OPUS_SET_SIGNAL_REQUEST: c_int = 4024;
pub const OPUS_GET_IN_DTX_REQUEST: c_int = 4049;

extern "C" {
    pub fn opus_encoder_create(
        fs: i32,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut OpusEncoder;
    pub fn opus_encoder_destroy(st: *mut OpusEncoder);
    pub fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    pub fn opus_encode_float(
        st: *mut OpusEncoder,
        pcm: *const f32,
        frame_size: c_int,
        data: *mut c_uchar,
        max_data_bytes: i32,
    ) -> c_int;

    pub fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    pub fn opus_decoder_destroy(st: *mut OpusDecoder);
    pub fn opus_decode_float(
        st: *mut OpusDecoder,
        data: *const c_uchar,
        len: i32,
        pcm: *mut f32,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
}
