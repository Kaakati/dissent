//! The computer's own sound, for a screen share.
//!
//! On Windows this is WASAPI process loopback (Windows 10 2004 and later), with this process and its
//! children excluded. So the share carries what other programs play, and never the call's voices or
//! this app's sounds, which would otherwise echo back into the call. Other platforms report it as
//! unavailable. macOS and Linux have native paths (ScreenCaptureKit, a PipeWire monitor stream), which are
//! not built, and a whole-device capture there would send the call back into itself.
//!
//! Nothing in the tests opens this: it would capture the machine's real sound. Tests use a synthetic source.

use crate::error::Result;

/// Whether this device can share its sound. When it cannot, [`SystemAudio::open`] fails too.
pub fn available() -> bool {
    imp::available()
}

/// A running capture of the computer's sound. Open and read it on one thread.
pub struct SystemAudio(imp::Capture);

impl SystemAudio {
    /// Starts capturing, or fails when the system refuses.
    pub fn open() -> Result<Self> {
        imp::Capture::open().map(Self)
    }

    /// Appends what was captured since the last call, as 48 kHz mono. Nothing is added while nothing plays.
    pub fn read_48k(&mut self, out: &mut Vec<f32>) -> Result<()> {
        self.0.read_48k(out)
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::error::{AudioError, Result};

    pub fn available() -> bool {
        false
    }

    pub enum Capture {}

    impl Capture {
        pub fn open() -> Result<Self> {
            Err(AudioError::Device("sharing the computer's sound is not available on this system".into()))
        }

        pub fn read_48k(&mut self, _out: &mut Vec<f32>) -> Result<()> {
            match *self {}
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::ManuallyDrop;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::core::{implement, Interface, Ref, Result as WinResult, HRESULT};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Media::Audio::{
        ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, IAgileObject, IAgileObject_Impl, BLOB, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId};
    use windows::Win32::System::Variant::VT_BLOB;

    use crate::error::{AudioError, Result};

    const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    const WAVE_FORMAT_PCM: u16 = 1;
    /// Process loopback needs Windows 10 2004, build 19041.
    const FIRST_BUILD: u32 = 19_041;
    /// How much the system buffers for us, in 100 ns units: 100 ms.
    const BUFFER_100NS: i64 = 1_000_000;
    const ACTIVATE_WAIT: Duration = Duration::from_secs(5);

    fn err(e: impl std::fmt::Display) -> AudioError {
        AudioError::Device(format!("computer sound: {e}"))
    }

    #[repr(C)]
    struct OsVersion {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        extra: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersion) -> i32;
    }

    pub fn available() -> bool {
        let mut info = OsVersion { size: std::mem::size_of::<OsVersion>() as u32, major: 0, minor: 0, build: 0, platform: 0, extra: [0; 128] };
        // SAFETY: `info` is an RTL_OSVERSIONINFOW with its size set, which the call fills in.
        let status = unsafe { RtlGetVersion(&mut info) };
        status == 0 && info.build >= FIRST_BUILD
    }

    /// Takes the one call Windows makes when the audio client is ready.
    // Agile, because Windows makes the call on a thread of its own, and refuses a handler that is not.
    #[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
    struct Activated(mpsc::SyncSender<WinResult<IAudioClient>>);

    impl IAgileObject_Impl for Activated_Impl {}

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        fn ActivateCompleted(&self, operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> WinResult<()> {
            let result = (|| {
                let operation = operation.ok()?;
                let mut status = HRESULT(0);
                let mut client = None;
                // SAFETY: both out pointers are valid for the call.
                unsafe { operation.GetActivateResult(&mut status, &mut client)? };
                status.ok()?;
                client.ok_or_else(windows::core::Error::empty)?.cast::<IAudioClient>()
            })();
            let _ = self.0.try_send(result);
            Ok(())
        }
    }

    pub struct Capture {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        event: HANDLE,
    }

    // SAFETY: the COM objects were made on a multithreaded apartment, which allows any thread to use them.
    unsafe impl Send for Capture {}

    impl Capture {
        pub fn open() -> Result<Self> {
            if !available() {
                return Err(err("this needs Windows 10 version 2004 or later"));
            }
            // SAFETY: joins this thread to the multithreaded apartment. A thread already in it gets S_FALSE.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let client = activate().map_err(err)?;
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM,
                nChannels: CHANNELS,
                nSamplesPerSec: crate::codec::SAMPLE_RATE,
                nAvgBytesPerSec: crate::codec::SAMPLE_RATE * u32::from(CHANNELS * BITS / 8),
                nBlockAlign: CHANNELS * BITS / 8,
                wBitsPerSample: BITS,
                cbSize: 0,
            };
            // SAFETY: `format` lives for the call, and the client copies it. The event is ours to close.
            unsafe {
                client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        BUFFER_100NS,
                        0,
                        &format,
                        None,
                    )
                    .map_err(err)?;
                let event = CreateEventW(None, false, false, None).map_err(err)?;
                client.SetEventHandle(event).map_err(err)?;
                let capture: IAudioCaptureClient = client.GetService().map_err(err)?;
                client.Start().map_err(err)?;
                Ok(Self { client, capture, event })
            }
        }

        pub fn read_48k(&mut self, out: &mut Vec<f32>) -> Result<()> {
            // SAFETY: each buffer is read for the frames GetBuffer reports, as 16-bit stereo, the format
            // the client was given, and is released before the next one.
            unsafe {
                while self.capture.GetNextPacketSize().map_err(err)? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).map_err(err)?;
                    let samples = std::slice::from_raw_parts(data.cast::<i16>(), frames as usize * usize::from(CHANNELS));
                    out.extend(samples.chunks_exact(usize::from(CHANNELS)).map(|lr| (f32::from(lr[0]) + f32::from(lr[1])) / 65_536.0));
                    self.capture.ReleaseBuffer(frames).map_err(err)?;
                }
            }
            Ok(())
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            // SAFETY: the client and the event are open until here, and the event is closed once.
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }

    fn activate() -> std::result::Result<IAudioClient, String> {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            ..Default::default()
        };
        // SAFETY: the process id is a plain read, and the loopback params are the variant the type above selects.
        unsafe {
            params.Anonymous.ProcessLoopbackParams.TargetProcessId = GetCurrentProcessId();
            params.Anonymous.ProcessLoopbackParams.ProcessLoopbackMode = PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE;
        }
        // Not dropped: clearing a PROPVARIANT frees its blob, and this blob points at `params` on the stack.
        let mut variant = ManuallyDrop::new(PROPVARIANT::default());
        // SAFETY: a default variant is empty, and this sets it to a blob over `params`, which outlives the wait below.
        unsafe {
            let inner = &mut *variant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: (&raw mut params).cast(),
            };
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
        // SAFETY: the path is a constant, and the variant is valid until the wait below ends.
        let _operation = unsafe {
            ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*variant as *const PROPVARIANT), &handler)
        }
        .map_err(|e| e.to_string())?;
        rx.recv_timeout(ACTIVATE_WAIT)
            .map_err(|_| "Windows did not answer".to_string())?
            .map_err(|e| e.to_string())
    }
}
