//! OS media integration (layer L5): hardware video decoding through macOS's codecs.
//!
//! [`register`] puts the VideoToolbox decoder factory in front of WaltCut's own decoders
//! (`waltcut_codecs::register_video_decoder`) for H.264 (`avcC`) and HEVC (`hvcC`) streams, 8- and
//! 10-bit, 4:2:0 and 4:2:2. Export uses the built-in Rust H.264 and ProRes 4444 encoders.
//!
//! Hardware decoding never makes a file undecodable:
//!
//! - the factory declines (falls through to the software decoder) when Settings ▸ Playback ▸
//!   Hardware decoding is Off (`waltcut_codecs::hw::set_hardware_decoding`), when the stream's
//!   format is one the hardware path does not take, or when the OS cannot create a hardware
//!   session for it (profile, size, no hardware decoder);
//! - a decoder that fails mid-stream switches to the software decoder transparently
//!   ([`HybridDecoder`]) and logs it.
//!
//! Pictures are the software decoder's: same planes (bit-exact on the parity fixtures), colour,
//! pixel aspect, pts and presentation order, so the two are interchangeable.
//!
//! This is the one crate allowed to use `unsafe` (OS FFI), and only in its FFI modules
//! (docs/adr/0001-platform-ffi.md).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod hybrid;
pub mod pen;
#[allow(unsafe_code)]
pub mod videotoolbox;

pub use hybrid::HybridDecoder;

/// What [`register`] made available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// A hardware decoder factory was registered (its name).
    Available(&'static str),
    /// Nothing to register on this system (why).
    Unavailable(&'static str),
}

/// Register the hardware video decoders (call once at startup; repeated calls are harmless).
/// Streams they do not take, and every stream while hardware decoding is Off, keep using
/// WaltCut's own decoders.
pub fn register() -> Availability {
    waltcut_codecs::register_video_decoder(videotoolbox_factory);
    waltcut_codecs::hw::set_hw_backend("VideoToolbox");
    Availability::Available("VideoToolbox")
}

/// Whether [`register`] has put a hardware decoder factory in front of our decoders.
pub fn registered() -> bool {
    waltcut_codecs::video_decoder_registered(videotoolbox_factory)
}

/// Whether VideoToolbox takes the stream of `entry` (whatever the Hardware decoding setting
/// says): diagnostics and tests.
pub fn hardware_decoder_for(entry: &waltcut_isobmff::SampleEntry) -> bool {
    waltcut_codecs::hw::NalStreamInfo::from_entry(entry).and_then(|r| r.ok()).is_some_and(|info| videotoolbox::VtDecoder::new(info).is_ok())
}

/// The VideoToolbox factory: a [`HybridDecoder`] around [`videotoolbox::VtDecoder`] for `avcC` /
/// `hvcC` streams VideoToolbox can decode in hardware, `None` otherwise.
pub fn videotoolbox_factory(entry: &waltcut_isobmff::SampleEntry) -> Option<waltcut_codecs::Result<Box<dyn waltcut_codecs::VideoDecoder>>> {
    if !waltcut_codecs::hw::hardware_decoding() {
        return None;
    }
    let info = waltcut_codecs::hw::NalStreamInfo::from_entry(entry)?.ok()?;
    match videotoolbox::VtDecoder::new(info.clone()) {
        Ok(vt) => Some(Ok(Box::new(HybridDecoder::new(Box::new(vt), entry.clone(), info)))),
        Err(why) => {
            log::info!("hardware decoding declined for {} video: {why}", entry.codec.name());
            waltcut_codecs::hw::note_hw_declined();
            None
        }
    }
}
