//! D3D11 device on the GPU that drives the chosen monitor, plus the GPU
//! colour converter / scaler (D3D11 Video Processor: BGRA → NV12 at any size).

use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::core::{Interface, Result};

pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter_name: String,
}

/// The adapter + HMONITOR for a monitor, found by its device name (\\.\DISPLAY1).
pub fn find_output(device_name: &str) -> Option<(IDXGIAdapter1, HMONITOR, String)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let name = adapter
                .GetDesc1()
                .map(|d| String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string())
                .unwrap_or_default();
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                if let Ok(d) = output.GetDesc() {
                    let dn = String::from_utf16_lossy(&d.DeviceName).trim_end_matches('\0').to_string();
                    if dn == device_name {
                        return Some((adapter, d.Monitor, name));
                    }
                }
                o += 1;
            }
            a += 1;
        }
    }
    None
}

impl Gpu {
    pub fn new(adapter: &IDXGIAdapter1, adapter_name: String) -> Result<Gpu> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter.cast::<IDXGIAdapter>()?,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device: ID3D11Device = device.unwrap();
        // Capture callbacks and the encoder use the device from different threads.
        unsafe {
            let mt: ID3D10Multithread = device.cast()?;
            let _ = mt.SetMultithreadProtected(true);
        }
        Ok(Gpu { device, context: context.unwrap(), adapter_name })
    }

    pub fn texture(&self, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut t = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t))? };
        Ok(t.unwrap())
    }

    /// Copy a texture back to the CPU as BGRA rows (tests / debugging only).
    pub fn read_bgra(&self, tex: &ID3D11Texture2D) -> Result<(u32, u32, Vec<u8>)> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut staging = None;
        unsafe { self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
        let staging = staging.unwrap();
        let mut out = vec![0u8; (desc.Width * desc.Height * 4) as usize];
        unsafe {
            self.context.CopyResource(&staging, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            for y in 0..desc.Height as usize {
                let src = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), desc.Width as usize * 4);
                out[y * desc.Width as usize * 4..][..desc.Width as usize * 4].copy_from_slice(src);
            }
            self.context.Unmap(&staging, 0);
        }
        Ok((desc.Width, desc.Height, out))
    }
}

/// BGRA (any size) → NV12 (output size) on the GPU's video engine.
pub struct Converter {
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    pub in_w: u32,
    pub in_h: u32,
    pub out_w: u32,
    pub out_h: u32,
}

impl Converter {
    /// `area`: the part of the input to use, (x, y, w, h) in pixels; None = all of it.
    pub fn new(gpu: &Gpu, in_w: u32, in_h: u32, out_w: u32, out_h: u32, fps: u32, area: Option<(u32, u32, u32, u32)>) -> Result<Converter> {
        let vdev: ID3D11VideoDevice = gpu.device.cast()?;
        let vctx: ID3D11VideoContext = gpu.context.cast()?;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
        };
        unsafe {
            let enumerator = vdev.CreateVideoProcessorEnumerator(&desc)?;
            let processor = vdev.CreateVideoProcessor(&enumerator, 0)?;
            // Desktop pixels are full-range sRGB; encoders expect BT.709 limited-range YUV.
            if let Ok(vctx1) = vctx.cast::<ID3D11VideoContext1>() {
                vctx1.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                vctx1.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            }
            // Never let the driver "auto-process" (no sharpening, no denoise).
            vctx.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let (ax, ay, aw, ah) = area.unwrap_or((0, 0, in_w, in_h));
            let src = RECT { left: ax as i32, top: ay as i32, right: (ax + aw).min(in_w) as i32, bottom: (ay + ah).min(in_h) as i32 };
            let dst = RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 };
            vctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
            vctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
            vctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst));
            Ok(Converter { vdev, vctx, enumerator, processor, in_w, in_h, out_w, out_h })
        }
    }

    /// Run one conversion: `src` (BGRA, in size) into `dst` (NV12, out size).
    pub fn convert(&self, src: &ID3D11Texture2D, dst: &ID3D11Texture2D) -> Result<()> {
        unsafe {
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
            };
            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut iv = None;
            self.vdev.CreateVideoProcessorInputView(src, &self.enumerator, &in_desc, Some(&mut iv))?;
            let mut ov = None;
            self.vdev.CreateVideoProcessorOutputView(dst, &self.enumerator, &out_desc, Some(&mut ov))?;
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: std::mem::ManuallyDrop::new(iv),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let streams = [stream];
            let r = self.vctx.VideoProcessorBlt(&self.processor, ov.as_ref().unwrap(), 0, &streams);
            // The stream struct holds a reference we must release ourselves.
            let [s] = streams;
            std::mem::ManuallyDrop::into_inner(s.pInputSurface);
            r
        }
    }
}
