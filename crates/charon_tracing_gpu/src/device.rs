use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
/// Facts for this device generation. `rtx` is true only after the hardware probe.
/// Adapter support and device enablement are deliberately not treated as readiness.
#[derive(Clone, Copy, Debug)]
pub struct DeviceCapabilities {
    pub pc: bool,
    pub mobile: bool,
    pub compute: bool,
    pub path: bool,
    pub lite_path: bool,
    pub rtx: bool,
    pub rtx_enabled: bool,
    pub rtx_adapter: Option<bool>,
    pub rtx_requires_recreation: bool,
}
impl DeviceCapabilities {
    pub fn query(device: &wgpu::Device) -> Self {
        let mobile = cfg!(any(target_os = "android", target_os = "ios"));
        let compute = device.limits().max_compute_workgroups_per_dimension > 0;
        let limits = device.limits();
        let enabled = !mobile
            && cfg!(feature = "rtx")
            && device.features().contains(rtx_features())
            && limits.max_acceleration_structures_per_shader_stage > 0
            && limits.max_blas_geometry_count > 0
            && limits.max_blas_primitive_count > 0
            && limits.max_tlas_instance_count > 0;
        Self {
            pc: !mobile,
            mobile,
            compute,
            path: !mobile && compute,
            lite_path: compute,
            rtx: false,
            rtx_enabled: enabled,
            rtx_adapter: None,
            rtx_requires_recreation: false,
        }
    }
}
/// No file, UI, automatic restart or default policy. A new device generation
/// starts unverified even if the prior process/device successfully used RTX.
#[derive(Clone)]
pub struct LightingDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_rtx: Option<bool>,
    verified: Arc<AtomicBool>,
}
impl LightingDevice {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            adapter_rtx: None,
            verified: Arc::new(AtomicBool::new(false)),
        }
    }
    fn with_adapter_features(mut self, features: wgpu::Features) -> Self {
        self.adapter_rtx = Some(features.contains(rtx_features()));
        self
    }
    pub fn with_adapter(self, features: wgpu::Features, info: &wgpu::AdapterInfo) -> Self {
        let hardware = matches!(
            info.device_type,
            wgpu::DeviceType::DiscreteGpu
                | wgpu::DeviceType::IntegratedGpu
                | wgpu::DeviceType::VirtualGpu
        );
        let mut value = self.with_adapter_features(features);
        if !hardware {
            value.adapter_rtx = Some(false);
        }
        value
    }
    pub fn info(&self) -> DeviceCapabilities {
        let mut info = DeviceCapabilities::query(&self.device);
        info.rtx_adapter = self.adapter_rtx;
        info.rtx_requires_recreation =
            cfg!(feature = "rtx") && self.adapter_rtx == Some(true) && !info.rtx_enabled && info.pc;
        info.rtx = info.rtx_enabled
            && self.adapter_rtx == Some(true)
            && self.verified.load(Ordering::Acquire);
        info
    }
    pub async fn probe_rtx(&self) -> Result<bool, String> {
        if !self.info().rtx_enabled || self.adapter_rtx != Some(true) {
            return Ok(false);
        }
        if self.verified.load(Ordering::Acquire) {
            return Ok(true);
        }
        #[cfg(feature = "rtx")]
        {
            let ready = rtx::verify_device(&self.device, &self.queue).await?;
            self.verified.store(ready, Ordering::Release);
            Ok(ready)
        }
        #[cfg(not(feature = "rtx"))]
        {
            Ok(false)
        }
    }
    pub fn supports(&self, mode: Mode, platform: Platform) -> bool {
        if mode.validate(platform).is_err() {
            return false;
        }
        let info = self.info();
        match mode {
            Mode::Rtx => info.rtx,
            Mode::Path => info.path,
            Mode::LitePath => info.lite_path,
            Mode::Legacy => true,
        }
    }
    pub async fn prepare(
        &self,
        geometry: Geometry,
        mode: Mode,
        platform: Platform,
        quality: Quality,
    ) -> Result<GpuTrace, String> {
        GpuTrace::new(
            self.device.clone(),
            self.queue.clone(),
            geometry,
            mode,
            platform,
            quality,
        )
        .await
    }
    pub(crate) fn raw_device(&self) -> &wgpu::Device {
        &self.device
    }
}
