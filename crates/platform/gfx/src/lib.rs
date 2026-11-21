//! Direct3D 8 (Xbox flavour) replacement on wgpu.
//!
//! The crate has three layers; see `docs/graphics/graphics.md` for the design.
//!
//! 1. **Pure Xbox data handling**, no GPU needed and fully unit-tested:
//!    [`format`] (the Xbox `D3DFORMAT` set), [`swizzle`] (Morton addressing),
//!    [`dxt`] (CPU DXT decoder), [`convert`] (texel expansion) and
//!    [`texture`] (header parsing, mip/cube/volume layout, the decode
//!    pipeline).
//! 2. **State model**: [`state`] (typed render/texture-stage state),
//!    [`vertex`] (primitive types, FVF, declarations), `translate` (state to
//!    wgpu descriptors) and `pipeline` (cache keyed by translated state).
//! 3. **Presentation**: `context`, `presenter`, `renderer` (with
//!    `clear` for partial clears and `program` for translated shader
//!    programs), `upload`.
//!    None of it knows about windows; a platform layer hands in a
//!    `wgpu::Surface`.

#[cfg(feature = "gpu")]
pub mod blit;
#[cfg(feature = "gpu")]
pub mod churn;
#[cfg(feature = "gpu")]
pub mod clear;
#[cfg(feature = "gpu")]
pub mod compile;
#[cfg(feature = "gpu")]
pub mod context;
pub mod convert;
#[cfg(feature = "gpu")]
pub mod depth_bits;
#[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
pub(crate) mod depth_fill;
#[cfg(feature = "gpu")]
pub mod resize;
#[cfg(feature = "gpu")]
pub mod surface_alias;
pub mod dxt;
#[cfg(feature = "gpu")]
pub mod encode;
pub mod external;
pub mod raster;
pub mod format;
#[cfg(feature = "gpu")]
pub mod guest_words;
#[cfg(feature = "gpu")]
pub mod occlusion;
#[cfg(feature = "gpu")]
pub mod persistent;
#[cfg(feature = "gpu")]
pub mod pipeline;
#[cfg(feature = "gpu")]
pub mod presenter;
#[cfg(feature = "gpu")]
pub mod program;
#[cfg(feature = "gpu")]
pub mod readback;
#[cfg(feature = "gpu")]
pub mod renderer;
#[cfg(feature = "gpu")]
pub mod repack;
#[cfg(feature = "gpu")]
pub mod ring;
pub mod state;
#[cfg(feature = "gpu")]
pub mod surface;
pub mod swizzle;
pub mod texture;
#[cfg(feature = "gpu")]
pub mod translate;
#[cfg(feature = "gpu")]
pub mod upload;
pub mod vertex;
pub mod xdk;

#[cfg(feature = "gpu")]
pub use context::{GfxContext, GfxError, default_backends, instance};
pub use format::{D3DFormat, HostFormat};
#[cfg(feature = "gpu")]
pub use presenter::{Acquired, OffscreenTarget, Presenter, RenderTarget};
#[cfg(feature = "gpu")]
pub use clear::MaskedClear;
#[cfg(feature = "gpu")]
pub use renderer::{BoundGroup, Draw, DrawViewport, RecordedDraw, Renderer, ScissorRect, VertexStream};
pub use state::{Clear, RenderState};
pub use texture::{DecodeOptions, TextureDesc, decode_texture};
