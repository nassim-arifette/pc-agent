//! In-process Microsoft desktop SAPI, never Windows.Media web dictation.
//! The PCM input is an anonymous memory stream, not a file or a shared recognizer.
use crate::{ErrorCode, Language, Result};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    core::{w, Interface, GUID, PCWSTR, PWSTR},
    Win32::{
        Media::{Audio::WAVEFORMATEX, Speech::*},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            StructuredStorage::CreateStreamOnHGlobal, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
            STREAM_SEEK_SET,
        },
    },
};

struct Apartment;
impl Apartment {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: this worker initializes COM once and owns all its COM interfaces.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: paired with successful CoInitializeEx on this worker.
        unsafe {
            CoUninitialize();
        }
    }
}

pub(super) struct Recognizer {
    token: ISpObjectToken,
    language: String,
    // Drop COM interfaces before uninitializing the worker apartment.
    _apartment: Apartment,
}
impl Recognizer {
    pub fn new(language: Language) -> Result<Self> {
        Self::create(language).map_err(|_| ErrorCode::EngineUnavailable.into())
    }
    fn create(language: Language) -> windows::core::Result<Self> {
        let apartment = Apartment::new()?;
        // SAFETY: all calls use owned COM objects on this initialized apartment. Returned
        // CoTaskMem strings are copied before freeing; no raw pointer escapes this scope.
        unsafe {
            let category: ISpObjectTokenCategory =
                CoCreateInstance(&SpObjectTokenCategory, None, CLSCTX_INPROC_SERVER)?;
            category.SetId(
                w!("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Recognizers"),
                false,
            )?;
            let tokens = category.EnumTokens(w!("Vendor=Microsoft"), PCWSTR::null())?;
            let mut count = 0;
            tokens.GetCount(&mut count)?;
            for i in 0..count {
                let token = tokens.Item(i)?;
                let attrs = token.OpenKey(w!("Attributes"))?;
                let codes = attrs.GetStringValue(w!("Language"))?;
                let languages = codes.to_string();
                CoTaskMemFree(Some(codes.0.cast()));
                let languages = languages?;
                let code = languages
                    .split(';')
                    .filter_map(|s| u16::from_str_radix(s, 16).ok())
                    .find_map(|id| match (id & 0x3ff, language) {
                        (9, Language::Auto | Language::En) => Some("en"),
                        (12, Language::Auto | Language::Fr) => Some("fr"),
                        _ => None,
                    });
                if let Some(code) = code {
                    let this = Self {
                        token,
                        language: code.into(),
                        _apartment: apartment,
                    };
                    // Fail before capture if the installed engine has no free dictation.
                    let recognizer = this.engine()?;
                    let context = recognizer.CreateRecoContext()?;
                    let grammar = context.CreateGrammar(1)?;
                    grammar.LoadDictation(PCWSTR::null(), SPLO_STATIC)?;
                    return Ok(this);
                }
            }
        }
        Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80004005u32 as i32,
        )))
    }
    fn engine(&self) -> windows::core::Result<ISpRecognizer> {
        // SAFETY: the token comes only from Microsoft's desktop SAPI registry and is used
        // on its owning COM thread. No shared recognizer/profile or audio persistence.
        unsafe {
            let engine: ISpRecognizer =
                CoCreateInstance(&SpInprocRecognizer, None, CLSCTX_INPROC_SERVER)?;
            engine.SetRecognizer(&self.token)?;
            engine.SetPropertyNum(w!("AdaptationOn"), 0)?;
            Ok(engine)
        }
    }
    pub fn transcribe(&self, samples: &[f32], cancel: &AtomicBool) -> Result<(String, String)> {
        self.recognize(samples, cancel)
            .map(|text| (text, self.language.clone()))
            .map_err(|_| {
                if cancel.load(Ordering::Acquire) {
                    ErrorCode::Cancelled.into()
                } else {
                    ErrorCode::RecognitionFailed.into()
                }
            })
    }
    fn recognize(&self, samples: &[f32], cancel: &AtomicBool) -> windows::core::Result<String> {
        // SAFETY: valid owned in-memory PCM buffers, checked byte counts, initialized COM
        // apartment, and interface lifetimes that outlive every SAPI read.
        unsafe {
            let memory = CreateStreamOnHGlobal(Default::default(), true)?;
            let pcm: Vec<u8> = samples
                .iter()
                .flat_map(|s| ((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes())
                .collect();
            let mut written = 0;
            memory
                .Write(pcm.as_ptr().cast(), pcm.len() as u32, Some(&mut written))
                .ok()?;
            if written as usize != pcm.len() {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )));
            }
            memory.Seek(0, STREAM_SEEK_SET, None)?;
            let format = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: 1,
                nSamplesPerSec: 16000,
                nAvgBytesPerSec: 32000,
                nBlockAlign: 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let stream: ISpStream = CoCreateInstance(&SpStream, None, CLSCTX_INPROC_SERVER)?;
            // SPDFID_WaveFormatEx, from the SAPI SDK.
            let format_id = GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);
            stream.SetBaseStream(&memory, &format_id, &format)?;
            let recognizer = self.engine()?;
            recognizer.SetInput(&stream, true)?;
            let context = recognizer.CreateRecoContext()?;
            context.SetAudioOptions(SPAO_NONE, std::ptr::null(), std::ptr::null())?;
            // SAPI requires the reserved interest bits in addition to requested events.
            let interest = (1u64 << 30)
                | (1u64 << 33)
                | (1u64 << SPEI_RECOGNITION.0)
                | (1u64 << SPEI_END_SR_STREAM.0);
            context.SetInterest(interest, interest)?;
            let grammar = context.CreateGrammar(1)?;
            grammar.LoadDictation(PCWSTR::null(), SPLO_STATIC)?;
            grammar.SetDictationState(SPRS_ACTIVE)?;
            recognizer.SetRecoState(SPRST_ACTIVE_ALWAYS)?;
            let mut text = String::new();
            let started = Instant::now();
            let result = loop {
                if cancel.load(Ordering::Acquire) || started.elapsed() > Duration::from_secs(60) {
                    break Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x80004004u32 as i32,
                    )));
                }
                let mut event = SPEVENT::default();
                let mut fetched = 0;
                if let Err(error) = context.GetEvents(1, &mut event, &mut fetched) {
                    break Err(error);
                }
                if fetched == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                let event_id = event._bitfield & 0xffff;
                let kind = (event._bitfield >> 16) & 0xffff;
                // SAPI transfers the event's COM reference to the receiver; from_raw
                // adopts exactly that reference, whose Drop releases it even on error.
                if kind == SPET_LPARAM_IS_OBJECT.0 && event.lParam.0 != 0 {
                    let object = windows::core::IUnknown::from_raw(event.lParam.0 as *mut _);
                    if event_id == SPEI_RECOGNITION.0 {
                        let result: ISpRecoResult = object.cast()?;
                        let mut words = PWSTR::null();
                        result.GetText(0, u32::MAX, true, &mut words, None)?;
                        let decoded = words.to_string();
                        CoTaskMemFree(Some(words.0.cast()));
                        let decoded = decoded?;
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(decoded.trim());
                    }
                } else if matches!(kind, 3 | 4) && event.lParam.0 != 0 {
                    CoTaskMemFree(Some(event.lParam.0 as *const _));
                }
                if event_id == SPEI_END_SR_STREAM.0 {
                    break Ok(text);
                }
            };
            let _ = grammar.SetDictationState(SPRS_INACTIVE);
            let _ = recognizer.SetRecoState(SPRST_INACTIVE_WITH_PURGE);
            result
        }
    }
}
