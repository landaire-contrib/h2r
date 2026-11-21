//! Exact producer scopes bound to D3D's transactional DrawAdmission.
//! Both helpers publish one packet after their final producer work. Drain
//! before that store and at its epilogue; nothing can escape as "next quad".
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::{
    DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalTarget, ExternalWrites,
};
use d3d::backend::{ExternalAttachments, ExternalDraws};
use engine::memory::GuestMemory;
use render_bridge::native_particles::{Call, Host, Producer};
use gfx::vertex::{PrimitiveType, VertexDataType};
use native_render::effect_geometry::{self, DynamicCall};
use native_render::effect_immediate::{self, Site};
use native_render::particle_gpu::*;
use native_render::particles::{self, Declaration, DrawIdentity, Vertex};

use crate::native_lights_glue::DeviceMemory;
use timeline::native::{Metric, Stage};

#[derive(Clone)]
enum Source {
    Particle(Call),
    Dynamic(DynamicCall, Vec<u8>),
    Immediate(Site),
}
impl Source {
    fn producer(&self) -> Producer {
        match self {
            Self::Particle(_) => Producer::Particle,
            Self::Dynamic(..) => Producer::Dynamic,
            Self::Immediate(site) => Producer::Immediate(*site),
        }
    }
}
#[derive(Clone)]
struct Pending {
    source: Source,
    id: u64,
}
#[derive(Default)]
struct Scope {
    waiting: Option<Pending>,
    issuing: Option<Pending>,
}
type Renderers = Arc<Mutex<HashMap<wgpu::Device, (u64, ParticleRenderer)>>>;
struct Admission {
    scope: Mutex<Scope>,
    renderers: Renderers,
    serial: AtomicU64,
}
struct ImmediateAdmission<B: d3d::backend::Backend + 'static> {
    device: d3d::abi::SharedDevice<B>,
    admission: Arc<Admission>,
}
struct Read<'a>(&'a dyn GuestMemory);
impl native_render::camera::GuestRead for Read<'_> {
    fn read_u32(&self, a: u32) -> Option<u32> {
        self.0.u32(a).ok()
    }
}
impl<B: d3d::backend::Backend + 'static> ImmediateAdmission<B> {
    fn arm(&self, source: Source) -> bool {
        let mut s = self
            .admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // A missed completion never grants ownership to a subsequent call.
        s.issuing = None;
        s.waiting = Some(Pending {
            source,
            id: self.admission.serial.fetch_add(1, Ordering::Relaxed),
        });
        true
    }
}
impl<B: d3d::backend::Backend + 'static> Host for ImmediateAdmission<B> {
    fn try_emit(&self, mem: &mut dyn GuestMemory, call: &Call) -> bool {
        let data = [[0i16, 0, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0], [0, 1, 0, 0]]
            .into_iter().flatten().flat_map(i16::to_le_bytes).collect();
        let carried = call.emission.attributes.map(|r| r.map(|v| v.map(f32::from_bits)));
        let pending = Pending {
            source: Source::Particle(call.clone()),
            id: self.admission.serial.fetch_add(1, Ordering::Relaxed),
        };
        let candidate = Direct { admission: &self.admission, pending, stage: Stage::Particles };
        self.device.lock().is_ok_and(|mut device| {
            device.try_native_inline_draw(
                &mut DeviceMemory(mem), PrimitiveType::QuadList, data, &carried, &candidate,
            ).ok() == Some(true)
        })
    }

    fn try_dynamic(&self, mem: &mut dyn GuestMemory, esp: u32, data: Option<u32>) -> bool {
        let pending = self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).waiting.clone();
        let Some(mut pending) = pending else { return false };
        let Source::Dynamic(call, bytes) = &mut pending.source else { return false };
        let data = match (call.kind, data) {
            (effect_geometry::Kind::EffectQuad, None) => call.context,
            (effect_geometry::Kind::Contrail, Some(data)) => data,
            _ => return false,
        };
        Stage::Effects.count(Metric::WalkExamined, 1);
        if effect_geometry::read_call(&Read(mem), esp, call.stride) != Ok(*call) {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        }
        let Ok(generated) = effect_geometry::read_generated(&Read(mem), data, call) else {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        };
        let Some(primitive) = PrimitiveType::from_xbox(call.primitive) else {
            Stage::Effects.count(Metric::WalkRefusedInputs, 1);
            return false;
        };
        *bytes = generated.clone();
        let candidate = Direct { admission: &self.admission, pending, stage: Stage::Effects };
        let drawn = self.device.lock().is_ok_and(|mut device| {
            device.try_native_inline_draw(
                &mut DeviceMemory(mem), primitive, generated, &[None; 16], &candidate,
            ).ok() == Some(true)
        });
        if drawn {
            self.admission.scope.lock().unwrap_or_else(|e| e.into_inner()).waiting = None;
        }
        Stage::Effects.count(if drawn { Metric::WalkSuppressed } else { Metric::WalkRefusedReservation }, 1);
        drawn
    }

    fn arm(&self, _: &mut dyn GuestMemory, call: &Call) -> bool {
        self.arm(Source::Particle(call.clone()))
    }
    fn arm_dynamic(&self, _: &mut dyn GuestMemory, call: &DynamicCall) -> bool {
        self.arm(Source::Dynamic(*call, Vec::new()))
    }
    fn arm_effect(&self, mem: &mut dyn GuestMemory, site: Site) -> bool {
        self.arm(Source::Immediate(site));
        // All vertices are published, but END is not. Drain prior work
        // without ownership, then own only this leaf's completed draw.
        self.publish(mem, Producer::Immediate(site), 0);
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing
            .is_some()
    }
    fn publish(&self, mem: &mut dyn GuestMemory, producer: Producer, data: u32) {
        let mut device = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let pending = {
            let mut s = self
                .admission
                .scope
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            s.issuing = None;
            s.waiting.take().filter(|p| p.source.producer() == producer)
        };
        let Some(mut p) = pending else { return };
        if let Source::Dynamic(call, bytes) = &mut p.source {
            let Ok(generated) = effect_geometry::read_generated(&Read(mem), data, call) else {
                return;
            };
            *bytes = generated;
        }
        // The guest has not published this packet yet. Earlier draws have
        // no active owner, even if they have identical geometry and state.
        if device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() != Some(true) {
            return;
        }
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing = Some(p);
    }
    fn finish(&self, mem: &mut dyn GuestMemory, producer: Producer) {
        let mut device = self.device.lock().unwrap_or_else(|e| e.into_inner());
        let active = {
            let mut s = self
                .admission
                .scope
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            s.waiting = None; // allocation failure / skipped emission
            s.issuing
                .as_ref()
                .is_some_and(|p| p.source.producer() == producer)
        };
        if active {
            let _ = device.drain_admission_boundary(&mut DeviceMemory(mem));
        }
        self.admission
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing = None;
    }
}

pub fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| render_bridge::native_particles::on(&s)) {
        return;
    }
    let admission = Arc::new(Admission {
        scope: Mutex::new(Scope::default()),
        renderers: Default::default(),
        serial: AtomicU64::new(1),
    });
    device
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .add_draw_admission(admission.clone());
    render_bridge::native_particles::set_host(Arc::new(ImmediateAdmission {
        device: device.clone(),
        admission,
    }));
}

/// Exact source bytes and declaration, independently checked against the
/// resolved draw. No matching by material or primitive shape alone.
fn geometry(
    draw: &d3d::ResolvedDraw,
    source: &Source,
) -> Result<(Vec<Vertex>, Option<RawVertices>, Vec<u32>), Refused> {
    if draw.indices.is_some() || draw.layout.pretransformed {
        return Err(Refused::Geometry);
    }
    let [layout] = draw.layout.streams.as_slice() else {
        return Err(Refused::Geometry);
    };
    let [stream] = draw.streams.as_slice() else {
        return Err(Refused::Geometry);
    };
    if layout.stream != 0 || layout.stride != stream.stride {
        return Err(Refused::Geometry);
    }
    match source {
        Source::Immediate(site) => {
            if stream.stride != 256 {
                return Err(Refused::Effect("ImmediateDeclaration"));
            }
            let mut written = 0u16;
            for a in &layout.attributes {
                let r = a.register.0;
                if r >= 16
                    || a.data_type != VertexDataType::Float4
                    || a.offset != u32::from(r) * 16
                    || written & (1 << r) != 0
                {
                    return Err(Refused::Effect("ImmediateDeclaration"));
                }
                written |= 1 << r;
            }
            if written != draw.shader.provided_inputs {
                return Err(Refused::Effect("ImmediateDeclaration"));
            }
            let (vertices, indices) = effect_immediate::decode(
                *site,
                draw.primitive.xbox_value(),
                draw.vertex_count,
                &stream.data,
                written,
                draw.shader.vertex_defaults,
            )
            .map_err(|e| Refused::Effect(e.name()))?;
            Ok((vertices, None, indices))
        }
        Source::Particle(call) => {
            let [a] = layout.attributes.as_slice() else {
                return Err(Refused::Geometry);
            };
            if draw.primitive != PrimitiveType::QuadList
                || draw.vertex_count != 4
                || stream.stride != 8
                || stream.data.len() != 32
                || a.offset != 0
                || a.register.0 != 0
                || a.data_type != VertexDataType::Short4
                || draw.shader.provided_inputs != 1
            {
                return Err(Refused::Geometry);
            }
            let expected = call
                .emission
                .vertices(Declaration::CornerShort4, draw.shader.vertex_defaults)
                .map_err(|_| Refused::Geometry)?;
            let actual: Vec<_> = stream
                .data
                .chunks_exact(8)
                .map(|bytes| {
                    let mut registers = draw.shader.vertex_defaults;
                    registers[0] = std::array::from_fn(|i| {
                        i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as f32
                    });
                    Vertex { registers }
                })
                .collect();
            if particles::differing_registers(&expected, &actual).map_err(|_| Refused::Geometry)?
                != 0
            {
                return Err(Refused::Geometry);
            }
            Ok((expected.to_vec(), None, particles::QUAD_INDICES.to_vec()))
        }
        Source::Dynamic(call, bytes) => {
            let primitive = match call.primitive {
                6 => PrimitiveType::TriangleStrip,
                7 => PrimitiveType::TriangleFan,
                8 => PrimitiveType::QuadList,
                _ => return Err(Refused::Geometry),
            };
            if draw.primitive != primitive
                || draw.vertex_count != call.vertices
                || stream.stride != call.stride
                || stream.data.as_ref() != bytes.as_slice()
            {
                return Err(Refused::Geometry);
            }
            let attributes = layout
                .attributes
                .iter()
                .map(|a| {
                    let format = match a.data_type {
                        VertexDataType::Float3 => wgpu::VertexFormat::Float32x3,
                        VertexDataType::Float2 => wgpu::VertexFormat::Float32x2,
                        VertexDataType::D3dColor => wgpu::VertexFormat::Unorm8x4Bgra,
                        _ => return Err(Refused::Geometry),
                    };
                    Ok(wgpu::VertexAttribute {
                        format,
                        offset: a.offset as u64,
                        shader_location: a.register.0 as u32,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let raw = RawVertices::new(call.kind, bytes.clone(), attributes)?;
            let mask = layout
                .attributes
                .iter()
                .try_fold(0u16, |mask, a| {
                    (a.register.0 < 16).then(|| mask | (1u16 << a.register.0))
                })
                .ok_or(Refused::Geometry)?;
            if mask != draw.shader.provided_inputs {
                return Err(Refused::Geometry);
            }
            Ok((
                Vec::new(),
                Some(raw),
                effect_geometry::indices(call).map_err(|_| Refused::Geometry)?,
            ))
        }
    }
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        // Consume even a mismatched first draw. A refused call cannot own
        // another packet; epilogue cancellation handles the zero-draw case.
        let pending = self
            .scope
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .issuing
            .take()?;
        self.plan(snapshot, &pending, Stage::Particles)
    }
}

struct Direct<'a> {
    admission: &'a Admission,
    pending: Pending,
    stage: Stage,
}

impl DrawAdmission for Direct<'_> {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        self.admission.plan(snapshot, &self.pending, self.stage)
    }
}

impl Admission {
    fn plan(
        &self,
        snapshot: &DrawSnapshot<'_>,
        pending: &Pending,
        stage: Stage,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let result = geometry(snapshot.draw, &pending.source);
        let (vertices, raw_vertices, indices) = match result {
            Ok(geometry) => geometry,
            Err(reason) => {
                static LOGGED: AtomicU64 = AtomicU64::new(0);
                if LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
                    eprintln!(
                        "native-particles-refused: producer={:?} reason={reason:?}",
                        pending.source.producer()
                    );
                }
                return None;
            }
        };
        let refusal = if snapshot.color_header == 0 {
            Some("MissingColorTarget")
        } else if snapshot.depth_header == 0 || snapshot.draw.target.depth_format.is_none() {
            Some("MissingDepthTarget")
        } else if snapshot.draw.clip.is_none() || !snapshot.draw.clip_rects.is_empty() {
            Some("UnsupportedClip")
        } else if snapshot.draw.glyph_cache != 0 {
            Some("GlyphFiltering")
        } else if snapshot.draw.hw.zpass_count_enable {
            Some("VisibilityQuery")
        } else if !snapshot.draw.state.bad.is_empty() {
            Some("UnresolvedState")
        } else {
            None
        };
        if let Some(reason) = refusal {
            static LOGGED: AtomicU64 = AtomicU64::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
                eprintln!(
                    "native-particles-refused: producer={:?} reason={reason}",
                    pending.source.producer()
                );
            }
            return None;
        }
        Some((
            ExternalPlan {
                stage,
                view: 4,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(Pass {
                draw: snapshot.draw.clone(),
                id: pending.id,
                color: snapshot.color_header,
                depth: snapshot.depth_header,
                vertices,
                raw_vertices,
                indices,
                renderers: self.renderers.clone(),
                prepared: Vec::new(),
            }),
        ))
    }
}

struct Pass {
    draw: d3d::ResolvedDraw,
    id: u64,
    color: u32,
    depth: u32,
    vertices: Vec<Vertex>,
    raw_vertices: Option<RawVertices>,
    indices: Vec<u32>,
    renderers: Renderers,
    prepared: Vec<(PreparedDraw, DrawIdentity, TargetKey)>,
}
/// What a snapshot needs of one target copy, whether it comes from an
/// isolated recording's `ExternalTarget` or the open pass's attachments.
struct CopyView<'a> {
    device: &'a wgpu::Device,
    guest_size: (u32, u32),
    size: (u32, u32),
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    stages: [Option<&'a wgpu::TextureView>; 4],
    copy: d3d::TargetCopy,
}
impl<'a> CopyView<'a> {
    fn of_target(t: &ExternalTarget<'a>) -> Self {
        Self {
            device: t.device,
            guest_size: t.guest_size,
            size: t.size,
            color_format: t.color_format,
            depth_format: t.depth_format,
            stages: t.stages,
            copy: t.copy,
        }
    }
    fn of_attachments(t: &'a ExternalAttachments, request: &d3d::ExternalPassRequest) -> Self {
        Self {
            device: &t.device,
            guest_size: request.surface_size,
            size: t.size,
            color_format: t.color_format,
            depth_format: t.depth_format,
            stages: t.stages.each_ref().map(Option::as_ref),
            copy: t.copy,
        }
    }
}
// `wgpu::Device` is a handle hashed and compared by identity, so its interior
// state never changes the key.
#[allow(clippy::mutable_key_type)]
fn renderer_for<'a>(
    renderers: &'a mut HashMap<wgpu::Device, (u64, ParticleRenderer)>,
    device: &wgpu::Device,
) -> &'a mut (u64, ParticleRenderer) {
    static DEVICES: AtomicU64 = AtomicU64::new(1);
    renderers.entry(device.clone()).or_insert_with(|| {
        let id = DEVICES.fetch_add(1, Ordering::Relaxed);
        (id, ParticleRenderer::new(id))
    })
}
impl Pass {
    fn snapshot(
        &self,
        t: &CopyView<'_>,
        renderer: &mut ParticleRenderer,
        device_id: u64,
    ) -> Result<ImmediateDrawSnapshot, Refused> {
        let draw = self.draw.for_copy(t.copy);
        let d = draw.as_ref();
        if t.guest_size != (d.target.width, d.target.height) {
            return Err(Refused::Target);
        }
        let scale = d3d::scaled::RenderScale::between(t.guest_size, t.size).ok_or(Refused::Target)?;
        let caps = gfx::translate::Capabilities::from_features(t.device.features());
        let program = d3d::nv2a::resolved::resolve(d, t.stages.map(|v| v.is_some()), caps, scale)
            .map_err(Refused::Shader)?;
        let mut microcode = vec![0x78, 0x20];
        let count = u16::try_from(program.words.len() / 4).map_err(|_| Refused::Geometry)?;
        microcode.extend(count.to_le_bytes());
        for word in &program.words {
            microcode.extend(word.to_le_bytes());
        }
        let mut render = d.state.render;
        if program.pixel_options.w_buffer {
            render.solid_offset_enable = false;
            render.wireframe_offset_enable = false;
            render.point_offset_enable = false;
        }
        let mut primitive =
            gfx::translate::primitive_state(&render, d.primitive.host_topology(), caps);
        primitive.strip_index_format = (primitive.topology
            == wgpu::PrimitiveTopology::TriangleStrip)
            .then_some(wgpu::IndexFormat::Uint32);
        let clip = d.display_rect(d.clip.ok_or(Refused::Scissor)?);
        let scissor = scale.rect(clip.x, clip.y, clip.width, clip.height);
        let mut textures = [None, None, None, None];
        for (n, slot) in textures.iter_mut().enumerate() {
            if let (Some(view), Some(dim), Some(header)) = (
                t.stages[n],
                program.pixel_options.texture_dims[n],
                d.textures[n],
            ) {
                *slot = Some(TextureBinding {
                    device: device_id,
                    generation: self.id,
                    identity: header as u64,
                    dimension: dim,
                    view: view.clone(),
                    sampler: renderer.sampler(t.device, &program.samplers[n]),
                });
            }
        }
        Ok(ImmediateDrawSnapshot {
            identity: DrawIdentity {
                device: device_id,
                generation: self.id,
                view: 4,
                call: self.id,
                copy: u32::from(t.copy == d3d::TargetCopy::Display),
            },
            target: TargetKey {
                color: self.color as u64,
                depth: self.depth as u64,
                size: t.size,
                color_format: t.color_format,
                depth_format: t.depth_format,
            },
            shader: ShaderSnapshot {
                microcode,
                pixel: program.pixel,
                pixel_options: program.pixel_options,
                flat_shading: program.vertex_options.flat_shading,
            },
            vertex_uniforms: program.vertex_uniforms,
            pixel_uniforms: program.pixel_uniforms,
            textures,
            vertices: self.vertices.clone(),
            raw_vertices: self.raw_vertices.clone(),
            indices: self.indices.clone(),
            primitive,
            depth: gfx::translate::depth_stencil_state(
                &render,
                Some(t.depth_format),
                d.primitive.host_topology(),
            )
            .ok_or(Refused::State)?,
            blend: gfx::translate::blend_state(&render),
            color_writes: gfx::translate::color_writes(render.color_write),
            blend_constant: gfx::translate::blend_constant(&render),
            stencil_reference: render.stencil.reference as u32,
            scissor,
        })
    }
}
impl ExternalPass for Pass {
    fn encode(&mut self, _: ExternalTarget<'_>) {} // only transactional recording is supported
    fn writes(&self) -> ExternalWrites {
        let r = &self.draw.state.render;
        ExternalWrites {
            color: r.color_write.xbox_value() != 0,
            depth_stencil: (r.depth_mode != gfx::state::DepthBufferMode::Disabled && r.depth_write)
                || r.stencil.enable,
        }
    }
    fn prepare_copy(&mut self, target: &ExternalTarget<'_>) -> Result<(), &'static str> {
        self.prepared
            .retain(|p| p.1.copy != u32::from(target.copy == d3d::TargetCopy::Display));
        let renderers = self.renderers.clone();
        let mut renderers = renderers.lock().unwrap_or_else(|e| e.into_inner());
        let (id, renderer) = renderer_for(&mut renderers, target.device);
        let refusal = |why: Refused| {
            static LOGGED: AtomicU64 = AtomicU64::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 16 {
                eprintln!(
                    "native-particles-refused: call={} copy={:?} reason={why:?}",
                    self.id, target.copy
                );
            }
            "particle resources"
        };
        let snapshot = self
            .snapshot(&CopyView::of_target(target), renderer, *id)
            .map_err(&refusal)?;
        let draw = renderer
            .prepare(target.device, &snapshot)
            .map_err(refusal)?;
        self.prepared
            .push((draw, snapshot.identity, snapshot.target));
        Ok(())
    }
    /// One packet appended to the D3D pass open on these attachments: no
    /// command buffer or render pass of its own. `None` (the isolated
    /// recording below) only for what a packet cannot carry.
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let view = CopyView::of_attachments(t, &request);
        // Sampling the colour or depth being drawn is a feedback loop the
        // open pass cannot express.
        let attached = [t.color.texture(), t.depth.texture()];
        if view.stages.iter().flatten().any(|v| attached.contains(&v.texture())) {
            return Some(Err("particle samples its own target"));
        }
        let renderers = self.renderers.clone();
        let mut renderers = renderers.lock().unwrap_or_else(|e| e.into_inner());
        let (id, renderer) = renderer_for(&mut renderers, &t.device);
        let refusal = |why: Refused| {
            static LOGGED: AtomicU64 = AtomicU64::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 16 {
                eprintln!(
                    "native-particles-refused: call={} copy={:?} reason={why:?}",
                    self.id, t.copy
                );
            }
            "particle resources"
        };
        let snapshot = match self.snapshot(&view, renderer, *id) {
            Ok(s) => s,
            Err(why) => return Some(Err(refusal(why))),
        };
        if snapshot.stencil_reference > u32::from(u8::MAX) {
            return None;
        }
        Some(
            match renderer.draws(&t.device, &t.queue, &t.uploads, &snapshot) {
                Ok((packet, discards)) => Ok(ExternalDraws {
                    ranges: 1,
                    packets: vec![packet],
                    discards,
                    writes: Vec::new(),
                }),
                Err(why) => Err(refusal(why)),
            },
        )
    }
    fn record_ranges(&mut self, target: ExternalTarget<'_>) -> usize {
        let Some(i) = self
            .prepared
            .iter()
            .position(|p| p.1.copy == u32::from(target.copy == d3d::TargetCopy::Display))
        else {
            return 0;
        };
        let (draw, identity, key) = self.prepared.swap_remove(i);
        if key.size != target.size
            || key.color_format != target.color_format
            || key.depth_format != target.depth_format
            || identity.copy != u32::from(target.copy == d3d::TargetCopy::Display)
        {
            return 0;
        }
        usize::from(
            draw.record(target.encoder, identity, key, target.color, target.depth)
                .is_ok(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d3d::backend::{StreamBytes, TargetInfo, Viewport};
    use d3d::shader::{ShaderInputs, VertexProcessing};
    use d3d::state::StateWords;
    use gfx::vertex::{StreamLayout, VertexAttribute, VertexLayout, VertexRegister};

    fn particle() -> (d3d::ResolvedDraw, Source) {
        let words = StateWords::defaults(false);
        let mut attributes = [None; 16];
        attributes[3] = Some([0.25f32.to_bits(), 0, 0, 1f32.to_bits()]);
        let source = Source::Particle(Call {
            esp: 0x100,
            return_address: 0x200,
            emission: particles::Emission { attributes },
        });
        let mut defaults = [[0., 0., 0., 1.]; 16];
        defaults[3] = [0.25, 0., 0., 1.];
        let data: Vec<u8> = [[0i16, 0, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0], [0, 1, 0, 0]]
            .into_iter()
            .flatten()
            .flat_map(i16::to_le_bytes)
            .collect();
        (
            d3d::ResolvedDraw {
                entry: "synthetic particle",
                primitive: PrimitiveType::QuadList,
                vertex_count: 4,
                layout: VertexLayout {
                    streams: vec![StreamLayout {
                        stream: 0,
                        stride: 8,
                        attributes: vec![VertexAttribute {
                            register: VertexRegister(0),
                            data_type: VertexDataType::Short4,
                            offset: 0,
                        }],
                    }],
                    pretransformed: false,
                },
                streams: vec![StreamBytes {
                    data: data.into(),
                    stride: 8,
                }],
                indices: None,
                state: words.translate(&gfx::xdk::RenderStateTable::default()),
                extras: words.stage_extras(),
                raw: Arc::new(words),
                target: TargetInfo {
                    width: 64,
                    height: 64,
                    format: gfx::D3DFormat::A8R8G8B8,
                    depth_format: Some(gfx::D3DFormat::D24S8),
                },
                shader: ShaderInputs {
                    vertex: VertexProcessing::Program {
                        start: 0,
                        microcode: Vec::<u32>::new().into(),
                    },
                    provided_inputs: 1,
                    vertex_defaults: defaults,
                    constants: Arc::new([[0.; 4]; 192]),
                    pixel_shader: Some(Arc::new([0; 60])),
                    combiner_state: Arc::new([0; 57]),
                    final_combiner_latch: true,
                    affine_projection: false,
                },
                viewport: Viewport {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                    min_z: 0.,
                    max_z: 1.,
                },
                clip: Some(d3d::PixelRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                }),
                clip_rects: Vec::new(),
                hw: Default::default(),
                texture_info: [None; 4],
                textures: [None; 4],
                retail_constants: None,
                ui_fit: None,
                glyph_cache: 0,
            },
            source,
        )
    }
    #[test]
    fn particle_matches_live_defaults_and_declared_corner_fetch() {
        let (mut draw, source) = particle();
        assert!(geometry(&draw, &source).is_ok());
        draw.shader.vertex_defaults[3][0] = 0.5;
        assert!(matches!(geometry(&draw, &source), Err(Refused::Geometry)));
        let (mut draw, source) = particle();
        draw.layout.streams[0].attributes[0].data_type = VertexDataType::NormShort4;
        assert!(matches!(geometry(&draw, &source), Err(Refused::Geometry)));
    }
    #[test]
    fn a_mismatch_consumes_the_scope_and_cannot_take_the_following_quad() {
        let (draw, source) = particle();
        let admission = Admission {
            scope: Mutex::new(Scope::default()),
            renderers: Default::default(),
            serial: AtomicU64::new(1),
        };
        fn snapshot(draw: &d3d::ResolvedDraw) -> DrawSnapshot<'_> {
            DrawSnapshot {
                draw,
                vertex_shader: 0,
                pixel_shader: 0,
                bound_textures: [0; 4],
                sampled_stages: [false; 4],
                palettes: [0; 4],
                color_header: 1,
                depth_header: 2,
            }
        }
        assert!(
            admission.admit(&snapshot(&draw)).is_none(),
            "no active producer"
        );
        admission.scope.lock().unwrap().issuing = Some(Pending {
            source,
            id: 1,
        });
        let mut wrong = draw.clone();
        wrong.vertex_count = 3;
        assert!(admission.admit(&snapshot(&wrong)).is_none());
        assert!(
            admission.admit(&snapshot(&draw)).is_none(),
            "a later identical quad is not owned"
        );
    }
    #[test]
    fn immediate_effect_requires_exact_register_declaration_and_rasterization() {
        let (mut draw, _) = particle();
        let source = Source::Immediate(Site::WeatherLayer);
        draw.primitive = PrimitiveType::QuadStrip;
        draw.layout.streams[0].stride = 256;
        draw.layout.streams[0].attributes[0].data_type = VertexDataType::Float4;
        draw.streams[0] = StreamBytes {
            data: vec![0; 1024].into(),
            stride: 256,
        };
        let (v, raw, indices) = geometry(&draw, &source).unwrap();
        assert!(raw.is_none());
        assert_eq!(v[0].registers[3], draw.shader.vertex_defaults[3]);
        assert_eq!(indices, [0, 1, 3, 0, 3, 2]);
        draw.layout.streams[0].attributes[0].offset = 16;
        assert!(matches!(
            geometry(&draw, &source),
            Err(Refused::Effect("ImmediateDeclaration"))
        ));
        draw.layout.streams[0].attributes[0].offset = 0;
        draw.primitive = PrimitiveType::PointList;
        assert!(matches!(
            geometry(&draw, &Source::Immediate(Site::WeatherParticles)),
            Err(Refused::Effect("WeatherPointRasterization"))
        ));
        draw.primitive = PrimitiveType::LineList;
        let (_, _, indices) = geometry(&draw, &Source::Immediate(Site::WeatherParticles)).unwrap();
        assert_eq!(indices, [0, 1, 2, 3]);
        draw.shader.provided_inputs |= 1 << 3;
        assert!(matches!(
            geometry(&draw, &Source::Immediate(Site::WeatherParticles)),
            Err(Refused::Effect("ImmediateDeclaration"))
        ));
    }
    #[test]
    fn raw_ribbon_bytes_and_live_register_mapping_must_match() {
        let (mut draw, _) = particle();
        let call = DynamicCall {
            kind: effect_geometry::Kind::Contrail,
            esp: 0x100,
            return_address: 0x200,
            builder: effect_geometry::CONTRAIL_BUILD,
            context: 0x300,
            vertices: 4,
            stride: 24,
            primitive: 6,
        };
        let bytes = vec![0; 96];
        let source = Source::Dynamic(call, bytes.clone());
        draw.primitive = PrimitiveType::TriangleStrip;
        draw.shader.provided_inputs = (1 << 0) | (1 << 9) | (1 << 3);
        draw.layout.streams[0] = StreamLayout {
            stream: 0,
            stride: 24,
            attributes: vec![
                VertexAttribute {
                    register: VertexRegister(0),
                    data_type: VertexDataType::Float3,
                    offset: 0,
                },
                VertexAttribute {
                    register: VertexRegister(9),
                    data_type: VertexDataType::Float2,
                    offset: 12,
                },
                VertexAttribute {
                    register: VertexRegister(3),
                    data_type: VertexDataType::D3dColor,
                    offset: 20,
                },
            ],
        };
        draw.streams[0] = StreamBytes {
            data: bytes.clone().into(),
            stride: 24,
        };
        let (_, raw, indices) = geometry(&draw, &source).unwrap();
        assert_eq!(raw.unwrap().bytes(), bytes);
        assert_eq!(indices, [0, 1, 2, 3]);
        let mut changed = bytes;
        changed[95] = 1;
        draw.streams[0].data = changed.into();
        assert!(matches!(geometry(&draw, &source), Err(Refused::Geometry)));
    }
    #[test]
    fn resolved_program_keeps_the_final_combiner_latch_and_w_clip_without_depth_writes() {
        let (mut draw, _) = particle();
        draw.hw.control0 = Some(d3d::nv2a::CONTROL0_Z_PERSPECTIVE);
        draw.state.render.depth_mode = gfx::state::DepthBufferMode::Disabled;
        let p =
            d3d::nv2a::resolved::resolve(&draw, [false; 4], gfx::translate::Capabilities::NONE, 2u32)
                .unwrap();
        assert!(p.pixel.explicit_final);
        assert!(p.vertex_uniforms.w_clip.is_some());
        assert!(!p.pixel_options.w_buffer);
        assert_eq!(p.vertex_uniforms.shift, [gfx::renderer::window_shift(1, 2); 2]);
    }
}
