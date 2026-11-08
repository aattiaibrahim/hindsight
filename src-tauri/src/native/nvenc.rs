//! NVIDIA NVENC, called directly (as OBS does): D3D11 NV12 textures in,
//! compressed AV1 / HEVC / H.264 packets out. Nothing leaves the GPU until the
//! finished packet is copied out.

use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr::null_mut;

use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{Interface, s, w};

use super::gpu::Gpu;
use super::nvenc_sys::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub fn from_setting(s: &str) -> Codec {
        match s {
            "h264" => Codec::H264,
            "hevc" => Codec::Hevc,
            _ => Codec::Av1,
        }
    }
    fn guid(self) -> GUID {
        match self {
            Codec::H264 => NV_ENC_CODEC_H264_GUID,
            Codec::Hevc => NV_ENC_CODEC_HEVC_GUID,
            Codec::Av1 => NV_ENC_CODEC_AV1_GUID,
        }
    }
    /// FFmpeg demuxer name for the raw stream this produces.
    pub fn raw_format(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "obu",
        }
    }
}

pub struct Packet {
    pub index: u64,
    pub key: bool,
    pub data: Vec<u8>,
}

pub struct EncoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Constant-quality level (lower = better), same scale as the old CQ setting.
    pub quality: u32,
    /// Keyframe every this many frames (clips can start at any keyframe).
    pub gop: u32,
}

pub struct Encoder {
    api: NV_ENCODE_API_FUNCTION_LIST,
    enc: *mut c_void,
    inputs: Vec<(ID3D11Texture2D, NV_ENC_REGISTERED_PTR)>,
    bitstream: NV_ENC_OUTPUT_PTR,
    next: usize,
    frame: u64,
    pub cfg: EncoderConfig,
    /// Keeps the driver's encoder library loaded while this encoder lives.
    _lib: NvLib,
}

/// nvEncodeAPI64.dll, unloaded when the last user lets go. After an NVIDIA
/// driver update, the next encoder then loads the new driver's copy instead of
/// the old one still sitting in memory.
struct NvLib(windows::Win32::Foundation::HMODULE);

impl Drop for NvLib {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::FreeLibrary(self.0);
        }
    }
}

// The NVENC session is only used from the encoder thread; moving it there is fine.
unsafe impl Send for Encoder {}

fn check(what: &str, status: NVENCSTATUS) -> Result<(), String> {
    if status == NV_ENC_SUCCESS { Ok(()) } else { Err(format!("NVENC {what} failed (status {status})")) }
}

macro_rules! call {
    ($api:expr, $f:ident, $($arg:expr),*) => {{
        let f = $api.$f.ok_or(concat!("NVENC is missing ", stringify!($f)))?;
        check(stringify!($f), unsafe { f($($arg),*) })
    }};
}

impl Codec {
    pub fn setting(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        }
    }
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.Data1, a.Data2, a.Data3, a.Data4) == (b.Data1, b.Data2, b.Data3, b.Data4)
}

/// What this PC's NVIDIA GPU can encode, straight from the driver (asked once).
/// AV1 needs an RTX 40 series or newer; HEVC a GTX 950 / 10 series or newer.
/// Empty when there's no NVIDIA GPU or driver.
pub fn supported_codecs() -> Vec<Codec> {
    static FOUND: std::sync::OnceLock<(String, Vec<Codec>)> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| detect().unwrap_or_default()).1.clone()
}

/// The NVIDIA GPU's name ("" if none), from the same check.
pub fn nvidia_gpu_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| detect().map(|d| d.0).unwrap_or_default()).clone()
}

fn detect() -> Result<(String, Vec<Codec>), String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.to_string())?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.VendorId != 0x10DE {
                continue; // not NVIDIA
            }
            let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
            let gpu = Gpu::new(&adapter, name.clone()).map_err(|e| e.to_string())?;
            let (_lib, api) = load_api()?;
            let mut enc: *mut c_void = null_mut();
            let mut open: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = zeroed();
            open.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
            open.deviceType = NV_ENC_DEVICE_TYPE_DIRECTX;
            open.device = gpu.device.as_raw();
            open.apiVersion = NVENCAPI_VERSION;
            call!(api, nvEncOpenEncodeSessionEx, &mut open, &mut enc)?;
            let mut n = 0u32;
            let counted = call!(api, nvEncGetEncodeGUIDCount, enc, &mut n);
            let mut guids: Vec<GUID> = vec![zeroed(); n as usize];
            let mut got = 0u32;
            let listed = counted.and_then(|_| call!(api, nvEncGetEncodeGUIDs, enc, guids.as_mut_ptr(), n, &mut got));
            if let Some(f) = api.nvEncDestroyEncoder {
                f(enc);
            }
            listed?;
            guids.truncate(got as usize);
            let codecs = [Codec::Av1, Codec::Hevc, Codec::H264]
                .into_iter()
                .filter(|c| guids.iter().any(|g| same_guid(g, &c.guid())))
                .collect();
            return Ok((name, codecs));
        }
        Err("No NVIDIA GPU found".into())
    }
}

/// The encoder to really use for a setting: "auto" is the best this GPU has
/// (AV1, then HEVC, then H.264), and a choice the GPU can't do falls back the
/// same way, so an older card never fails to record.
pub fn effective_encoder(setting: &str) -> &'static str {
    pick(setting, &supported_codecs())
}

pub fn pick(setting: &str, supported: &[Codec]) -> &'static str {
    if supported.is_empty() {
        // Nothing to go on (no NVIDIA driver): keep the old behaviour.
        return if setting == "auto" { "av1" } else { Codec::from_setting(setting).setting() };
    }
    if setting != "auto" {
        let want = Codec::from_setting(setting);
        if supported.contains(&want) {
            return want.setting();
        }
    }
    [Codec::Av1, Codec::Hevc, Codec::H264].into_iter().find(|c| supported.contains(c)).unwrap_or(Codec::H264).setting()
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn auto_and_unsupported_choices_fall_back() {
        let rtx40 = [Codec::Av1, Codec::Hevc, Codec::H264];
        let rtx30 = [Codec::Hevc, Codec::H264];
        let old = [Codec::H264];
        assert_eq!(pick("auto", &rtx40), "av1");
        assert_eq!(pick("auto", &rtx30), "hevc");
        assert_eq!(pick("av1", &rtx30), "hevc", "AV1 on an RTX 30 records HEVC instead");
        assert_eq!(pick("av1", &old), "h264");
        assert_eq!(pick("h264", &rtx40), "h264", "an explicit choice the GPU can do is kept");
        assert_eq!(pick("auto", &[]), "av1");
    }

    /// cargo test live_gpu_codecs -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gpu_codecs() {
        println!("{}: {:?} -> auto = {}", nvidia_gpu_name(), supported_codecs(), effective_encoder("auto"));
        assert!(!supported_codecs().is_empty());
    }
}

fn load_api() -> Result<(NvLib, NV_ENCODE_API_FUNCTION_LIST), String> {
    unsafe {
        let lib = NvLib(LoadLibraryW(w!("nvEncodeAPI64.dll")).map_err(|_| "NVIDIA encoder not found (is the NVIDIA driver installed?)".to_string())?);
        let NvLib(lib_h) = &lib;
        let lib_h = *lib_h;
        let f = GetProcAddress(lib_h, s!("NvEncodeAPICreateInstance")).ok_or("NvEncodeAPICreateInstance missing")?;
        let create: unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS = std::mem::transmute(f);
        let mut list: NV_ENCODE_API_FUNCTION_LIST = zeroed();
        list.version = NV_ENCODE_API_FUNCTION_LIST_VER;
        check("create instance", create(&mut list))?;
        Ok((lib, list))
    }
}

impl Encoder {
    pub fn new(gpu: &Gpu, cfg: EncoderConfig) -> Result<Encoder, String> {
        let (lib, api) = load_api()?;
        let mut enc: *mut c_void = null_mut();
        let mut open: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = unsafe { zeroed() };
        open.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
        open.deviceType = NV_ENC_DEVICE_TYPE_DIRECTX;
        open.device = gpu.device.as_raw();
        open.apiVersion = NVENCAPI_VERSION;
        call!(api, nvEncOpenEncodeSessionEx, &mut open, &mut enc)?;

        let mut me = Encoder { api, enc, inputs: Vec::new(), bitstream: null_mut(), next: 0, frame: 0, cfg, _lib: lib };
        me.initialize()?;
        // A few input textures so the GPU converter and encoder never wait on each other.
        for _ in 0..3 {
            let tex = gpu
                .texture(me.cfg.width, me.cfg.height, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET)
                .map_err(|e| e.to_string())?;
            let mut reg: NV_ENC_REGISTER_RESOURCE = unsafe { zeroed() };
            reg.version = NV_ENC_REGISTER_RESOURCE_VER;
            reg.resourceType = NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            reg.width = me.cfg.width;
            reg.height = me.cfg.height;
            reg.resourceToRegister = tex.as_raw();
            reg.bufferFormat = NV_ENC_BUFFER_FORMAT_NV12;
            reg.bufferUsage = NV_ENC_INPUT_IMAGE;
            call!(me.api, nvEncRegisterResource, me.enc, &mut reg)?;
            me.inputs.push((tex, reg.registeredResource));
        }
        let mut bs: NV_ENC_CREATE_BITSTREAM_BUFFER = unsafe { zeroed() };
        bs.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        call!(me.api, nvEncCreateBitstreamBuffer, me.enc, &mut bs)?;
        me.bitstream = bs.bitstreamBuffer;
        Ok(me)
    }

    fn initialize(&mut self) -> Result<(), String> {
        let c = &self.cfg;
        let mut preset: NV_ENC_PRESET_CONFIG = unsafe { zeroed() };
        preset.version = NV_ENC_PRESET_CONFIG_VER;
        preset.presetCfg.version = NV_ENC_CONFIG_VER;
        call!(
            self.api,
            nvEncGetEncodePresetConfigEx,
            self.enc,
            c.codec.guid(),
            NV_ENC_PRESET_P4_GUID,
            NV_ENC_TUNING_INFO_HIGH_QUALITY,
            &mut preset
        )?;
        let mut config = preset.presetCfg;
        config.gopLength = c.gop;
        config.frameIntervalP = 1; // no B-frames: every packet is in display order
        // Constant quality: VBR with a quality target and no bitrate target.
        config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_VBR;
        config.rcParams.averageBitRate = 0;
        config.rcParams.maxBitRate = 0;
        config.rcParams.targetQuality = c.quality.clamp(1, 51) as u8;
        config.rcParams.set_enableAQ(1);
        unsafe {
            match c.codec {
                Codec::Av1 => {
                    let a = &mut config.encodeCodecConfig.av1Config;
                    a.idrPeriod = c.gop;
                    a.set_repeatSeqHdr(1);
                    a.colorPrimaries = 1; // BT.709
                    a.transferCharacteristics = 1;
                    a.matrixCoefficients = 1;
                    a.colorRange = 0; // limited (TV) range, what players expect
                }
                Codec::Hevc => {
                    let h = &mut config.encodeCodecConfig.hevcConfig;
                    h.idrPeriod = c.gop;
                    h.set_repeatSPSPPS(1);
                    let v = &mut h.hevcVUIParameters;
                    v.videoSignalTypePresentFlag = 1;
                    v.colourDescriptionPresentFlag = 1;
                    v.colourPrimaries = 1;
                    v.transferCharacteristics = 1;
                    v.colourMatrix = 1;
                    v.videoFullRangeFlag = 0;
                }
                Codec::H264 => {
                    let h = &mut config.encodeCodecConfig.h264Config;
                    h.idrPeriod = c.gop;
                    h.set_repeatSPSPPS(1);
                    let v = &mut h.h264VUIParameters;
                    v.videoSignalTypePresentFlag = 1;
                    v.colourDescriptionPresentFlag = 1;
                    v.colourPrimaries = 1;
                    v.transferCharacteristics = 1;
                    v.colourMatrix = 1;
                    v.videoFullRangeFlag = 0;
                }
            }
        }
        let mut init: NV_ENC_INITIALIZE_PARAMS = unsafe { zeroed() };
        init.version = NV_ENC_INITIALIZE_PARAMS_VER;
        init.encodeGUID = c.codec.guid();
        init.presetGUID = NV_ENC_PRESET_P4_GUID;
        init.encodeWidth = c.width;
        init.encodeHeight = c.height;
        init.darWidth = c.width;
        init.darHeight = c.height;
        init.frameRateNum = c.fps;
        init.frameRateDen = 1;
        init.enablePTD = 1;
        init.encodeConfig = &mut config;
        init.maxEncodeWidth = c.width;
        init.maxEncodeHeight = c.height;
        init.tuningInfo = NV_ENC_TUNING_INFO_HIGH_QUALITY;
        init.bufferFormat = NV_ENC_BUFFER_FORMAT_NV12;
        call!(self.api, nvEncInitializeEncoder, self.enc, &mut init)
    }

    /// The NV12 texture to fill (via the GPU converter) before calling `encode`.
    pub fn next_input(&self) -> &ID3D11Texture2D {
        &self.inputs[self.next].0
    }

    /// Encode the texture from `next_input()` as the next frame.
    pub fn encode(&mut self, force_key: bool) -> Result<Packet, String> {
        let reg = self.inputs[self.next].1;
        self.next = (self.next + 1) % self.inputs.len();

        let mut map: NV_ENC_MAP_INPUT_RESOURCE = unsafe { zeroed() };
        map.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
        map.registeredResource = reg;
        call!(self.api, nvEncMapInputResource, self.enc, &mut map)?;

        let mut pic: NV_ENC_PIC_PARAMS = unsafe { zeroed() };
        pic.version = NV_ENC_PIC_PARAMS_VER;
        pic.inputWidth = self.cfg.width;
        pic.inputHeight = self.cfg.height;
        pic.inputBuffer = map.mappedResource;
        pic.outputBitstream = self.bitstream;
        pic.bufferFmt = NV_ENC_BUFFER_FORMAT_NV12;
        pic.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
        pic.inputTimeStamp = self.frame;
        pic.frameIdx = self.frame as u32;
        if force_key {
            pic.encodePicFlags = (NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS) as u32;
        }
        let encoded = call!(self.api, nvEncEncodePicture, self.enc, &mut pic);

        let result = encoded.and_then(|_| {
            let mut lock: NV_ENC_LOCK_BITSTREAM = unsafe { zeroed() };
            lock.version = NV_ENC_LOCK_BITSTREAM_VER;
            lock.outputBitstream = self.bitstream;
            call!(self.api, nvEncLockBitstream, self.enc, &mut lock)?;
            let data = unsafe {
                std::slice::from_raw_parts(lock.bitstreamBufferPtr as *const u8, lock.bitstreamSizeInBytes as usize).to_vec()
            };
            let key = lock.pictureType == NV_ENC_PIC_TYPE_IDR || lock.pictureType == NV_ENC_PIC_TYPE_I;
            call!(self.api, nvEncUnlockBitstream, self.enc, self.bitstream)?;
            Ok(Packet { index: self.frame, key, data })
        });
        let _ = call!(self.api, nvEncUnmapInputResource, self.enc, map.mappedResource);
        self.frame += 1;
        result
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.bitstream.is_null()
                && let Some(f) = self.api.nvEncDestroyBitstreamBuffer
            {
                f(self.enc, self.bitstream);
            }
            if let Some(f) = self.api.nvEncUnregisterResource {
                for (_, r) in &self.inputs {
                    f(self.enc, *r);
                }
            }
            if let Some(f) = self.api.nvEncDestroyEncoder {
                f(self.enc);
            }
        }
    }
}
