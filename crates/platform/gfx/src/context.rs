//! Device acquisition. Windowing-agnostic: the only thing a platform layer
//! has to supply is a `wgpu::Surface` (or nothing, for headless use).

use crate::translate::Capabilities;

/// Errors from bringing up the GPU.
#[derive(Debug, thiserror::Error)]
pub enum GfxError {
    /// No adapter satisfied the request.
    #[error("no suitable GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    /// The adapter refused to create a device.
    #[error("device request failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    /// Surface creation failed.
    #[error("surface creation failed: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    /// Acquiring the next frame failed unrecoverably.
    #[error("surface error: {0}")]
    Frame(String),
}

/// Owns the wgpu instance, adapter, device and queue.
pub struct GfxContext {
    /// The instance (kept alive for the surface's lifetime).
    pub instance: wgpu::Instance,
    /// The chosen adapter.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The submission queue.
    pub queue: wgpu::Queue,
    /// Optional features that were enabled.
    pub caps: Capabilities,
}

impl GfxContext {
    /// Creates a context, preferring an adapter that can present to
    /// `compatible_surface`.
    pub async fn new(instance: wgpu::Instance, compatible_surface: Option<&wgpu::Surface<'_>>) -> Result<Self, GfxError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;
        let features = adapter.features() & Capabilities::wanted_features();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("gfx device"),
                required_features: features,
                ..Default::default()
            })
            .await?;
        Ok(Self { instance, adapter, device, queue, caps: Capabilities::from_features(features) })
    }

    /// Headless context (no surface), blocking.
    pub fn headless() -> Result<Self, GfxError> {
        pollster::block_on(Self::new(instance(), None))
    }
}

/// Backends used when `WGPU_BACKEND` is unset. Apple and Windows hosts keep
/// every backend; every other Unix asks for Vulkan only, so adapter selection
/// cannot land on GLES, which cannot present on Wayland without a display
/// handle.
pub fn default_backends() -> wgpu::Backends {
    if cfg!(any(target_vendor = "apple", windows)) { wgpu::Backends::all() } else { wgpu::Backends::VULKAN }
}

/// The wgpu instance every host window and headless context starts from:
/// `WGPU_BACKEND` when set, else [`default_backends`].
pub fn instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::from_env().unwrap_or_else(default_backends);
    wgpu::Instance::new(desc)
}
