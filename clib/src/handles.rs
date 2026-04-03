use std::{ffi::c_void, sync::Arc};
use xsynth_core::{
    channel_group::ChannelGroup,
    soundfont::{SampleSoundfont, SoundfontBase},
};
use xsynth_realtime::RealtimeSynth;

/// Handle of an internal ChannelGroup instance in XSynth.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct XSynth_ChannelGroup {
    pub group: *mut c_void,
}

impl XSynth_ChannelGroup {
    pub(crate) fn null() -> Self {
        Self {
            group: std::ptr::null_mut(),
        }
    }

    pub(crate) fn from(group: ChannelGroup) -> Self {
        let group = Box::into_raw(Box::new(group));
        Self {
            group: group as *mut c_void,
        }
    }

    pub(crate) fn try_drop(self) -> bool {
        if self.group.is_null() {
            return false;
        }
        let group = self.group as *mut ChannelGroup;
        unsafe { drop(Box::from_raw(group)) }
        true
    }

    pub(crate) fn try_as_ref(&self) -> Option<&ChannelGroup> {
        if self.group.is_null() {
            return None;
        }
        let group = self.group as *mut ChannelGroup;
        Some(unsafe { &*group })
    }

    #[allow(clippy::mut_from_ref)]
    pub(crate) fn try_as_mut(&self) -> Option<&mut ChannelGroup> {
        if self.group.is_null() {
            return None;
        }
        let group = self.group as *mut ChannelGroup;
        Some(unsafe { &mut *group })
    }

    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn as_mut_unchecked(&self) -> &mut ChannelGroup {
        let group = self.group as *mut ChannelGroup;
        unsafe { &mut *group }
    }
}

/// Handle of an internal Soundfont object in XSynth.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct XSynth_Soundfont {
    pub soundfont: *mut c_void,
}

impl XSynth_Soundfont {
    pub(crate) fn null() -> Self {
        Self {
            soundfont: std::ptr::null_mut(),
        }
    }

    pub(crate) fn from(sf: Arc<SampleSoundfont>) -> Self {
        let sf = Box::into_raw(Box::new(sf));
        Self {
            soundfont: sf as *mut c_void,
        }
    }

    pub(crate) fn try_drop(self) -> bool {
        if self.soundfont.is_null() {
            return false;
        }
        let soundfont = self.soundfont as *mut Arc<SampleSoundfont>;
        unsafe { drop(Box::from_raw(soundfont)) }
        true
    }

    pub(crate) fn try_clone(&self) -> Option<Arc<dyn SoundfontBase>> {
        if self.soundfont.is_null() {
            return None;
        }
        let sf = unsafe { &*(self.soundfont as *mut Arc<SampleSoundfont>) };
        Some(sf.clone())
    }
}

/// Handle of an internal RealtimeSynth instance in XSynth.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct XSynth_RealtimeSynth {
    pub synth: *mut c_void,
}

impl XSynth_RealtimeSynth {
    pub(crate) fn null() -> Self {
        Self {
            synth: std::ptr::null_mut(),
        }
    }

    pub(crate) fn from(synth: RealtimeSynth) -> Self {
        let synth = Box::into_raw(Box::new(synth));
        Self {
            synth: synth as *mut c_void,
        }
    }

    pub(crate) fn try_drop(self) -> bool {
        if self.synth.is_null() {
            return false;
        }
        let synth = self.synth as *mut RealtimeSynth;
        unsafe { drop(Box::from_raw(synth)) }
        true
    }

    pub(crate) fn try_as_ref(&self) -> Option<&RealtimeSynth> {
        if self.synth.is_null() {
            return None;
        }
        let synth = self.synth as *mut RealtimeSynth;
        Some(unsafe { &*synth })
    }

    #[allow(clippy::mut_from_ref)]
    pub(crate) fn try_as_mut(&self) -> Option<&mut RealtimeSynth> {
        if self.synth.is_null() {
            return None;
        }
        let synth = self.synth as *mut RealtimeSynth;
        Some(unsafe { &mut *synth })
    }

    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn as_mut_unchecked(&self) -> &mut RealtimeSynth {
        let synth = self.synth as *mut RealtimeSynth;
        unsafe { &mut *synth }
    }
}
