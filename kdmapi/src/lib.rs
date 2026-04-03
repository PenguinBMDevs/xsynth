#![allow(non_snake_case)]

use hotwatch::{Event, EventKind, Hotwatch};
use std::{
    ffi::c_void,
    os::raw::c_ulong,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};
use xsynth_core::channel::{ChannelConfigEvent, ChannelEvent};
use xsynth_realtime::{RealtimeEventSender, RealtimeSynth, SynthEvent};

#[cfg(windows)]
use winapi::{
    shared::{basetsd::DWORD_PTR, minwindef::DWORD, windef::HWND},
    um::{
        mmsystem::{
            CALLBACK_EVENT, CALLBACK_FUNCTION, CALLBACK_THREAD, CALLBACK_WINDOW, HMIDI, HMIDIOUT,
        },
        synchapi::SetEvent,
        winnt::HANDLE,
        winuser::{IsWindow, PostMessageW, PostThreadMessageW},
    },
};

mod parsers;
use parsers::*;

struct Synth {
    killed: Arc<AtomicBool>,
    stats_join_handle: Option<thread::JoinHandle<()>>,
    senders: RealtimeEventSender,
    hotwatch: Hotwatch,
    settings_path: PathBuf,
    soundfonts_path: PathBuf,

    // This field is necessary to keep the synth loaded
    _synth: RealtimeSynth,
}

// RealtimeSynth internally owns the CPAL stream. In KDMAPI we keep it only
// for ownership/lifetime; control flows through cloned senders and teardown.
struct SharedSynth(Synth);
unsafe impl Send for SharedSynth {}

struct GlobalState {
    synth: Mutex<Option<Box<SharedSynth>>>,
    sender: AtomicPtr<RealtimeEventSender>,
    current_voice_count: AtomicU64,
}

fn global_state() -> &'static GlobalState {
    static GLOBAL_STATE: OnceLock<GlobalState> = OnceLock::new();
    GLOBAL_STATE.get_or_init(|| GlobalState {
        synth: Mutex::new(None),
        sender: AtomicPtr::new(std::ptr::null_mut()),
        current_voice_count: AtomicU64::new(0),
    })
}

fn log_kdmapi_error(context: &str, err: impl std::fmt::Display) {
    eprintln!("xsynth-kdmapi: {context}: {err}");
}

fn teardown_synth(mut synth: Box<SharedSynth>) {
    let synth = &mut synth.0;
    synth.killed.store(true, Ordering::Release);
    if let Some(handle) = synth.stats_join_handle.take() {
        if handle.join().is_err() {
        eprintln!("xsynth-kdmapi: stats thread panicked during shutdown");
        }
    }

    if let Err(err) = synth.hotwatch.unwatch(&synth.settings_path) {
        log_kdmapi_error("failed to unwatch settings.json", err);
    }
    if let Err(err) = synth.hotwatch.unwatch(&synth.soundfonts_path) {
        log_kdmapi_error("failed to unwatch soundfonts.json", err);
    }

    match Config::<Settings>::new() {
        Ok(config) => {
            if let Err(err) = config.repair() {
                log_kdmapi_error("failed to repair settings.json", err);
            }
        }
        Err(err) => log_kdmapi_error("failed to resolve settings.json path", err),
    }

    match Config::<SFList>::new() {
        Ok(config) => {
            if let Err(err) = config.repair() {
                log_kdmapi_error("failed to repair soundfonts.json", err);
            }
        }
        Err(err) => log_kdmapi_error("failed to resolve soundfonts.json path", err),
    }
}

fn load_config<T>() -> Result<T, String>
where
    T: Default + serde::Serialize + for<'a> serde::Deserialize<'a> + ConfigPath,
{
    Config::<T>::new()?.load()
}

// region: Custom XSynth KDMAPI functions

/// This entire function is custom to xsynth and is not part of
/// the KDMAPI standard. Its basically just for testing.
#[no_mangle]
pub extern "C" fn GetVoiceCount() -> u64 {
    global_state().current_voice_count.load(Ordering::Relaxed)
}

// endregion

// region: KDMAPI functions

#[no_mangle]
pub extern "C" fn InitializeKDMAPIStream() -> i32 {
    let config = match load_config::<Settings>() {
        Ok(config) => config,
        Err(err) => {
            log_kdmapi_error("failed to load settings.json", err);
            return 0;
        }
    };
    let sflist = match load_config::<SFList>() {
        Ok(config) => config,
        Err(err) => {
            log_kdmapi_error("failed to load soundfonts.json", err);
            return 0;
        }
    };
    let settings_path = match Config::<Settings>::path() {
        Ok(path) => path,
        Err(err) => {
            log_kdmapi_error("failed to resolve settings.json path", err);
            return 0;
        }
    };
    let soundfonts_path = match Config::<SFList>::path() {
        Ok(path) => path,
        Err(err) => {
            log_kdmapi_error("failed to resolve soundfonts.json path", err);
            return 0;
        }
    };

    let realtime_synth = match catch_unwind(AssertUnwindSafe(|| {
        RealtimeSynth::open_with_default_output(config.get_synth_config())
    })) {
        Ok(synth) => synth,
        Err(_) => {
            eprintln!("xsynth-kdmapi: failed to open realtime synth");
            return 0;
        }
    };
    let mut sender = realtime_synth.get_sender_ref().clone();
    let params = realtime_synth.stream_params();

    sender.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
        ChannelConfigEvent::SetLayerCount(config.get_layers()),
    )));
    sender.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
        ChannelConfigEvent::SetSoundfonts(sflist.create_sfbase_vector(params)),
    )));

    let killed = Arc::new(AtomicBool::new(false));

    let stats = realtime_synth.get_stats();
    let voice_count = &global_state().current_voice_count;

    let killed_thread = killed.clone();
    let stats_join_handle = thread::spawn(move || {
        while !killed_thread.load(Ordering::Acquire) {
            voice_count.store(stats.voice_count(), Ordering::Relaxed);
            thread::sleep(Duration::from_millis(10));
        }
    });

    let mut hotwatch = match Hotwatch::new_with_custom_delay(Duration::from_millis(500)) {
        Ok(hotwatch) => hotwatch,
        Err(err) => {
            log_kdmapi_error("failed to initialize file watcher", err);
            killed.store(true, Ordering::Release);
            let _ = stats_join_handle.join();
            return 0;
        }
    };

    // Watch for config changes and apply them
    let mut sender_thread = sender.clone();
    if let Err(err) = hotwatch.watch(settings_path.clone(), move |event: Event| {
            if let EventKind::Modify(_) = event.kind {
                thread::sleep(Duration::from_millis(10));
                match load_config::<Settings>() {
                    Ok(settings) => {
                        let layers = settings.get_layers();
                        sender_thread.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
                            ChannelConfigEvent::SetLayerCount(layers),
                        )));
                    }
                    Err(err) => log_kdmapi_error("failed to reload settings.json", err),
                }
            }
        }) {
        log_kdmapi_error("failed to watch settings.json", err);
        killed.store(true, Ordering::Release);
        let _ = stats_join_handle.join();
        return 0;
    }

    // Watch for soundfont list changes and apply them
    let mut sender_thread = sender.clone();
    if let Err(err) = hotwatch.watch(soundfonts_path.clone(), move |event: Event| {
            if let EventKind::Modify(_) = event.kind {
                thread::sleep(Duration::from_millis(10));
                match load_config::<SFList>() {
                    Ok(sflist) => {
                        let sfs = sflist.create_sfbase_vector(params);
                        sender_thread.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
                            ChannelConfigEvent::SetSoundfonts(sfs),
                        )));
                    }
                    Err(err) => log_kdmapi_error("failed to reload soundfonts.json", err),
                }
            }
        }) {
        log_kdmapi_error("failed to watch soundfonts.json", err);
        killed.store(true, Ordering::Release);
        let _ = stats_join_handle.join();
        return 0;
    }

    let mut new_synth = Box::new(SharedSynth(Synth {
        killed,
        senders: sender,
        stats_join_handle: Some(stats_join_handle),
        hotwatch,
        settings_path,
        soundfonts_path,
        _synth: realtime_synth,
    }));
    let sender_ptr = std::ptr::addr_of_mut!(new_synth.0.senders);

    let old_synth = {
        let state = global_state();
        let mut synth = state.synth.lock().unwrap();
        let old_synth = synth.replace(new_synth);
        state.sender.store(sender_ptr, Ordering::Release);
        old_synth
    };
    if let Some(old_synth) = old_synth {
        teardown_synth(old_synth);
    }
    1
}

#[no_mangle]
pub extern "C" fn TerminateKDMAPIStream() -> i32 {
    let synth = {
        let state = global_state();
        state.sender.store(std::ptr::null_mut(), Ordering::Release);
        state.synth.lock().unwrap().take()
    };
    if let Some(synth) = synth {
        teardown_synth(synth);
        global_state().current_voice_count.store(0, Ordering::Relaxed);
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn ResetKDMAPIStream() {
    if let Some(synth) = global_state().synth.lock().unwrap().as_mut() {
            synth.0.senders.reset_synth();
    }
}

#[no_mangle]
pub extern "C" fn SendDirectData(dwMsg: u32) -> u32 {
    let sender = global_state().sender.load(Ordering::Acquire);
    if sender.is_null() {
        0
    } else {
        unsafe {
            (*sender).send_event_u32(dwMsg);
        }
        1
    }
}

#[no_mangle]
pub extern "C" fn SendDirectDataNoBuf(dwMsg: u32) -> u32 {
    SendDirectData(dwMsg)
}

#[no_mangle]
pub extern "C" fn IsKDMAPIAvailable() -> u32 {
    (!global_state().sender.load(Ordering::Acquire).is_null()) as u32
}

#[no_mangle]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn ReturnKDMAPIVer(
    Major: *mut c_ulong,
    Minor: *mut c_ulong,
    Build: *mut c_ulong,
    Revision: *mut c_ulong,
) -> u32 {
    *Major = 4;
    *Minor = 1;
    *Build = 0;
    *Revision = 5;
    1
}

#[no_mangle]
pub extern "C" fn timeGetTime64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// endregion

// region: Unimplemented functions

#[no_mangle]
pub extern "C" fn DisableFeedbackMode() {}

#[no_mangle]
pub extern "C" fn SendCustomEvent(_eventtype: u32, _chan: u32, _param: u32) -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn SendDirectLongData() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn SendDirectLongDataNoBuf() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn PrepareLongData() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn UnprepareLongData() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn DriverSettings(
    _dwParam: c_ulong,
    _dwCmd: c_ulong,
    _lpValue: *mut c_void,
    _cbSize: c_ulong,
) -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn LoadCustomSoundFontsList(_Directory: u16) {}

#[no_mangle]
pub extern "C" fn GetDriverDebugInfo() {}

// endregion

// region: Callback functions for WINMM Wrapper (Windows Only)

cfg_if::cfg_if! {
  if #[cfg(windows)] {
    type CallbackFunction = unsafe extern "C" fn(HMIDIOUT, DWORD, DWORD_PTR, DWORD_PTR, DWORD_PTR);
    unsafe extern "C" fn def_callback(_: HMIDIOUT, _: DWORD, _: DWORD_PTR, _: DWORD_PTR, _: DWORD_PTR) {
    }

    #[derive(Clone, Copy)]
    struct CallbackState {
        dummy_device: HMIDI,
        callback_instance: DWORD_PTR,
        callback: CallbackFunction,
        callback_type: DWORD,
    }

    fn callback_state() -> &'static Mutex<CallbackState> {
        static CALLBACK_STATE: OnceLock<Mutex<CallbackState>> = OnceLock::new();
        CALLBACK_STATE.get_or_init(|| {
            Mutex::new(CallbackState {
                dummy_device: std::ptr::null_mut(),
                callback_instance: 0,
                callback: def_callback,
                callback_type: 0,
            })
        })
    }

    #[no_mangle]
    pub extern "C" fn modMessage() -> u32 {
        1
    }

    #[no_mangle]
    #[allow(clippy::missing_safety_doc)]
    pub unsafe extern "C" fn InitializeCallbackFeatures(
        OMHM: HMIDI,
        OMCB: CallbackFunction,
        OMI: DWORD_PTR,
        _OMU: DWORD_PTR,
        OMCM: DWORD,
    ) -> u32 {
        let mut state = callback_state().lock().unwrap();
        state.dummy_device = OMHM;
        state.callback = OMCB;
        state.callback_instance = OMI;
        state.callback_type = OMCM;

        #[allow(clippy::fn_address_comparisons)]
        if OMCM == CALLBACK_WINDOW && state.callback != def_callback && IsWindow(state.callback as HWND) != 0 {
            return 0;
        }

        1
    }

    #[no_mangle]
    #[allow(clippy::missing_safety_doc)]
    pub unsafe extern "C" fn RunCallbackFunction(Msg: DWORD, P1: DWORD_PTR, P2: DWORD_PTR) {
        let state = *callback_state().lock().unwrap();

        //We do a match case just to support stuff if needed
        match state.callback_type {
            CALLBACK_FUNCTION => {
                (state.callback)(
                    state.dummy_device as HMIDIOUT,
                    Msg,
                    P1,
                    P2,
                    state.callback_instance,
                );
            }
            CALLBACK_EVENT => {
                SetEvent(state.callback as HANDLE);
            }
            CALLBACK_THREAD => {
                #[allow(clippy::fn_to_numeric_cast_with_truncation)]
                if let Ok(p2) = P2.try_into() {
                    PostThreadMessageW(state.callback as DWORD, Msg, P1, p2);
                }
            }
            CALLBACK_WINDOW => {
                if let Ok(p2) = P2.try_into() {
                    PostMessageW(state.callback as HWND, Msg, P1, p2);
                }
            }
            _ => println!("Type was NULL, Do Nothing"),
        }
    }
  }
}

// endregion
