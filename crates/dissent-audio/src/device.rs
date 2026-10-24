//! Audio devices through cpal. Capture and playout run on the audio driver's callback threads.
//!
//! The callbacks never block, allocate or resample. They only move samples between the device
//! and a lock-free ring buffer. Resampling to and from 48 kHz runs on the pipeline's thread, in
//! [`CaptureStream::read_48k`] and [`PlayoutStream::write_48k`].
//!
//! Devices are opened at 48 kHz when they support it. Otherwise they open at their default rate.
//! Sample formats are f32 and i16. Other formats are rejected with [`AudioError::Device`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SupportedStreamConfig};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::codec::SAMPLE_RATE;
use crate::error::{AudioError, Result};
use crate::resample::LinearResampler;

/// Ring buffer length, in seconds of audio at the device rate.
const RING_SECONDS: usize = 1;

/// A device the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Name as the operating system reports it.
    pub name: String,
    /// True for the system default device in its direction.
    pub is_default: bool,
}

fn device_err(e: impl std::fmt::Display) -> AudioError {
    AudioError::Device(e.to_string())
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// Lists the capture devices on the default host.
pub fn list_inputs() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default = host.default_input_device().as_ref().and_then(device_name);
    let devices = host.input_devices().map_err(device_err)?;
    Ok(devices
        .filter_map(|d| device_name(&d))
        .map(|name| DeviceInfo {
            is_default: Some(&name) == default.as_ref(),
            name,
        })
        .collect())
}

/// Lists the playout devices on the default host.
pub fn list_outputs() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default = host.default_output_device().as_ref().and_then(device_name);
    let devices = host.output_devices().map_err(device_err)?;
    Ok(devices
        .filter_map(|d| device_name(&d))
        .map(|name| DeviceInfo {
            is_default: Some(&name) == default.as_ref(),
            name,
        })
        .collect())
}

fn find_input(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_input_device()
            .ok_or_else(|| AudioError::Device("no default input device".into())),
        Some(wanted) => host
            .input_devices()
            .map_err(device_err)?
            .find(|d| device_name(d).as_deref() == Some(wanted))
            .ok_or_else(|| AudioError::Device(format!("no input device named {wanted}"))),
    }
}

fn find_output(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host
            .default_output_device()
            .ok_or_else(|| AudioError::Device("no default output device".into())),
        Some(wanted) => host
            .output_devices()
            .map_err(device_err)?
            .find(|d| device_name(d).as_deref() == Some(wanted))
            .ok_or_else(|| AudioError::Device(format!("no output device named {wanted}"))),
    }
}

fn is_supported_format(f: SampleFormat) -> bool {
    matches!(f, SampleFormat::F32 | SampleFormat::I16)
}

/// Picks a 48 kHz f32 or i16 configuration if the device has one, else its default.
fn pick_config(
    ranges: Option<impl Iterator<Item = cpal::SupportedStreamConfigRange>>,
    default: SupportedStreamConfig,
) -> Result<SupportedStreamConfig> {
    if let Some(ranges) = ranges {
        for r in ranges {
            let fits = r.min_sample_rate() <= SAMPLE_RATE && r.max_sample_rate() >= SAMPLE_RATE;
            if fits && is_supported_format(r.sample_format()) {
                return Ok(r.with_sample_rate(SAMPLE_RATE));
            }
        }
    }
    if is_supported_format(default.sample_format()) {
        Ok(default)
    } else {
        Err(AudioError::Device(format!(
            "unsupported sample format {:?}",
            default.sample_format()
        )))
    }
}

/// Microphone input. Samples arrive in a ring buffer at the device rate.
pub struct CaptureStream {
    stream: cpal::Stream,
    consumer: HeapCons<f32>,
    resampler: LinearResampler,
    device_rate: u32,
    overflows: Arc<AtomicU64>,
    scratch: Vec<f32>,
}

impl CaptureStream {
    /// Opens the named input device, or the default device when `name` is `None`, and starts it.
    pub fn open(name: Option<&str>) -> Result<Self> {
        let device = find_input(name)?;
        let default = device.default_input_config().map_err(device_err)?;
        let ranges = device.supported_input_configs().ok();
        let config = pick_config(ranges, default)?;
        let device_rate = config.sample_rate();
        let channels = usize::from(config.channels().max(1));

        let ring = HeapRb::<f32>::new(device_rate as usize * RING_SECONDS);
        let (producer, consumer) = ring.split();
        let overflows = Arc::new(AtomicU64::new(0));
        let stream = build_input(&device, &config, producer, channels, overflows.clone())?;
        stream.play().map_err(device_err)?;
        Ok(CaptureStream {
            stream,
            consumer,
            resampler: LinearResampler::new(device_rate, SAMPLE_RATE),
            device_rate,
            overflows,
            scratch: Vec::new(),
        })
    }

    /// Sample rate the device delivers.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Drains captured audio, converts it to 48 kHz mono and appends it to `out`. Returns the
    /// number of samples appended. Call it from the pipeline thread, not from a callback.
    pub fn read_48k(&mut self, out: &mut Vec<f32>) -> usize {
        self.scratch.clear();
        let n = self.consumer.occupied_len();
        self.scratch.reserve(n);
        while let Some(s) = self.consumer.try_pop() {
            self.scratch.push(s);
        }
        let before = out.len();
        self.resampler.process(&self.scratch, out);
        out.len() - before
    }

    /// Samples the callback dropped because the ring was full. A growing count means the
    /// pipeline is not reading fast enough.
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }

    /// Stops the stream. It also stops when the value is dropped.
    pub fn stop(&self) -> Result<()> {
        self.stream.pause().map_err(device_err)
    }
}

fn build_input(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    producer: HeapProd<f32>,
    channels: usize,
    overflows: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let stream_config = config.config();
    let on_error = |_e: cpal::Error| {};
    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let mut producer = producer;
            device.build_input_stream::<f32, _, _>(
                stream_config,
                move |data: &[f32], _| {
                    for frame in data.chunks_exact(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32;
                        if producer.try_push(mono).is_err() {
                            overflows.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                on_error,
                None,
            )
        }
        SampleFormat::I16 => {
            let mut producer = producer;
            device.build_input_stream::<i16, _, _>(
                stream_config,
                move |data: &[i16], _| {
                    for frame in data.chunks_exact(channels) {
                        let sum: f32 = frame.iter().map(|&s| f32::from(s) / 32_768.0).sum();
                        if producer.try_push(sum / channels as f32).is_err() {
                            overflows.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                on_error,
                None,
            )
        }
        other => {
            return Err(AudioError::Device(format!(
                "unsupported sample format {other:?}"
            )))
        }
    };
    stream.map_err(device_err)
}

/// Speaker output. The pipeline writes 48 kHz audio, and the output callback reads it at the
/// device rate.
pub struct PlayoutStream {
    stream: cpal::Stream,
    producer: HeapProd<f32>,
    resampler: LinearResampler,
    device_rate: u32,
    underruns: Arc<AtomicU64>,
    scratch: Vec<f32>,
}

impl PlayoutStream {
    /// Opens the named output device, or the default device when `name` is `None`, and starts it.
    pub fn open(name: Option<&str>) -> Result<Self> {
        let device = find_output(name)?;
        let default = device.default_output_config().map_err(device_err)?;
        let ranges = device.supported_output_configs().ok();
        let config = pick_config(ranges, default)?;
        let device_rate = config.sample_rate();
        let channels = usize::from(config.channels().max(1));

        let ring = HeapRb::<f32>::new(device_rate as usize * RING_SECONDS);
        let (producer, consumer) = ring.split();
        let underruns = Arc::new(AtomicU64::new(0));
        let stream = build_output(&device, &config, consumer, channels, underruns.clone())?;
        stream.play().map_err(device_err)?;
        Ok(PlayoutStream {
            stream,
            producer,
            resampler: LinearResampler::new(SAMPLE_RATE, device_rate),
            device_rate,
            underruns,
            scratch: Vec::new(),
        })
    }

    /// Sample rate the device plays.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Converts 48 kHz mono audio to the device rate and queues it for playout. Samples that do
    /// not fit in the ring are dropped and counted as overflows.
    pub fn write_48k(&mut self, samples: &[f32]) {
        self.scratch.clear();
        self.resampler.process(samples, &mut self.scratch);
        for &s in &self.scratch {
            // A full ring drops the newest sample. The pipeline is then ahead of the device.
            let _ = self.producer.try_push(s);
        }
    }

    /// Callbacks that found the ring empty and played silence. A growing count means the pipeline
    /// is not keeping up with the device.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Stops the stream. It also stops when the value is dropped.
    pub fn stop(&self) -> Result<()> {
        self.stream.pause().map_err(device_err)
    }
}

fn build_output(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    consumer: HeapCons<f32>,
    channels: usize,
    underruns: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let stream_config = config.config();
    let on_error = |_e: cpal::Error| {};
    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let mut consumer = consumer;
            device.build_output_stream::<f32, _, _>(
                stream_config,
                move |data: &mut [f32], _| {
                    for frame in data.chunks_exact_mut(channels) {
                        let s = match consumer.try_pop() {
                            Some(s) => s,
                            None => {
                                underruns.fetch_add(1, Ordering::Relaxed);
                                0.0
                            }
                        };
                        frame.fill(s);
                    }
                },
                on_error,
                None,
            )
        }
        SampleFormat::I16 => {
            let mut consumer = consumer;
            device.build_output_stream::<i16, _, _>(
                stream_config,
                move |data: &mut [i16], _| {
                    for frame in data.chunks_exact_mut(channels) {
                        let s = match consumer.try_pop() {
                            Some(s) => (s.clamp(-1.0, 1.0) * 32_767.0) as i16,
                            None => {
                                underruns.fetch_add(1, Ordering::Relaxed);
                                0
                            }
                        };
                        frame.fill(s);
                    }
                },
                on_error,
                None,
            )
        }
        other => {
            return Err(AudioError::Device(format!(
                "unsupported sample format {other:?}"
            )))
        }
    };
    stream.map_err(device_err)
}
