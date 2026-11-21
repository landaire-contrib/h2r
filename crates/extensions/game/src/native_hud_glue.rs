//! Ordered handoff from the game's UI callbacks to the native UI renderer.
//! A single-draw pass retains its immutable packet as worker fallback, so a
//! direct producer commits without waiting for the render thread. A model
//! draw with sibling ranges keeps the guest code as its fallback.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws, ExternalPassRequest, IndexedStreams};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalWrites};
use engine::memory::GuestMemory;
use render_bridge::native_hud::{Host, HudVertices, OUTLINE_PUSH_BYTES, OutlineVertices, UiScope};
use gfx::vertex::PrimitiveType;
use native_render::ui_gpu::{Refusal, UiGpu};
use timeline::native::{Metric, Stage, Ui};

use crate::native_lights_glue::DeviceMemory;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scope {
    owner: u64,
    kind: UiScope,
    stack: u32,
    frame: u32,
    eligible: bool,
}

#[derive(Default)]
struct ScopeStack(Vec<Scope>);

impl ScopeStack {
    fn enter(&mut self, mut scope: Scope) {
        // The guest stack grows down. Discard any scope bypassed by an
        // unwind before inheriting ownership at the new callback.
        self.0.retain(|old| old.owner == scope.owner && old.stack > scope.stack);
        scope.eligible &= self.permits(scope.owner);
        self.0.push(scope);
    }

    fn active(&self, owner: u64) -> Option<Scope> {
        self.0.last().copied().filter(|scope| scope.owner == owner)
    }

    fn permits(&self, owner: u64) -> bool {
        self.active(owner).is_none_or(|scope| scope.eligible)
    }

    fn position(&self, scope: Scope) -> Option<usize> {
        self.0.iter().rposition(|s| {
            (s.owner, s.kind, s.stack) == (scope.owner, scope.kind, scope.stack)
        })
    }
}

thread_local! {
    static SCOPES: RefCell<ScopeStack> = RefCell::new(ScopeStack::default());
}

struct Admission {
    owner: u64,
    gpu: Arc<Mutex<UiGpu>>,
}

impl Admission {
    fn plan(
        &self,
        snapshot: &DrawSnapshot<'_>,
        frame: u32,
        siblings: Arc<[IndexedStreams]>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let draw = snapshot.draw;
        if snapshot.color_header == 0 {
            Ui::Target.count();
            Stage::Ui.count(Metric::RefusedResources, 1);
            return None;
        }
        Some((
            ExternalPlan {
                stage: Stage::Ui,
                view: 4,
                ranges: 1 + siblings.len(),
                slots: Vec::new(),
            },
            Box::new(UiPass {
                draw: Arc::new(draw.clone()),
                gpu: self.gpu.clone(),
                frame,
                siblings,
            }),
        ))
    }
}

impl DrawAdmission for Admission {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let scope = SCOPES.with(|scopes| scopes.borrow().active(self.owner))?;
        if scope.kind == UiScope::Model {
            Ui::ModelGuestBuilderDraw.count();
            return None;
        }
        Ui::GuestBuilderDraw.count();
        if !scope.eligible {
            return None;
        }
        self.plan(snapshot, scope.frame, Vec::new().into())
    }
}

struct Direct<'a> {
    admission: &'a Admission,
    frame: u32,
    siblings: Arc<[IndexedStreams]>,
}

impl<'a> Direct<'a> {
    fn new(admission: &'a Admission, frame: u32) -> Self {
        Self { admission, frame, siblings: Vec::new().into() }
    }
}

impl DrawAdmission for Direct<'_> {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        self.admission.plan(snapshot, self.frame, self.siblings.clone())
    }
}

struct UiPass {
    draw: Arc<d3d::ResolvedDraw>,
    gpu: Arc<Mutex<UiGpu>>,
    frame: u32,
    siblings: Arc<[IndexedStreams]>,
}

impl UiPass {
    fn prepare(&self, target: &ExternalAttachments, request: ExternalPassRequest)
        -> Result<ExternalDraws, Refusal>
    {
        if request.surface_size != (self.draw.target.width, self.draw.target.height) {
            return Err(Refusal::Target);
        }
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        let copy = self.draw.for_copy(target.copy);
        let mut total = gpu.prepare(target, copy.as_ref(), self.frame)?;
        for sibling in self.siblings.iter() {
            let mut draw = (*copy).clone();
            draw.layout = sibling.layout.clone();
            draw.vertex_count = sibling.vertex_count;
            draw.streams = sibling.streams.clone();
            draw.indices = Some(sibling.indices.clone());
            let part = gpu.prepare(target, &draw, self.frame)?;
            total.ranges += part.ranges;
            total.packets.extend(part.packets);
            total.discards |= part.discards;
        }
        Ok(total)
    }
}

impl ExternalPass for UiPass {
    fn requires_independent_samples(&self) -> bool {
        !self.siblings.is_empty()
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        Some(self.prepare(target, request).map_err(|reason| {
            match reason {
                Refusal::Geometry => Ui::Geometry,
                Refusal::Target => Ui::Target,
                Refusal::State => Ui::State,
                Refusal::Shader => Ui::Shader,
                Refusal::Texture => Ui::Texture,
                Refusal::Upload => Ui::Upload,
            }.count();
            "native UI draw"
        }))
    }

    /// The packet is the head draw alone, so sibling ranges have none.
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.siblings.is_empty().then(|| Arc::clone(&self.draw))
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: !gfx::translate::color_writes(self.draw.state.render.color_write).is_empty(),
            depth_stencil: self.draw.writes_depth_stencil(),
        }
    }

    fn colour_only(&self) -> bool {
        self.draw.target.depth_format.is_none()
    }
}

struct Glue<B: d3d::Backend> {
    device: d3d::abi::SharedDevice<B>,
    admission: Arc<Admission>,
}

impl<B: d3d::Backend> Glue<B> {
    fn fan<const N: usize>(
        &self,
        mem: &mut dyn GuestMemory,
        vertices: &[[(u8, [f32; 4]); N]; 4],
        push_bytes: Option<u32>,
    ) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            Ui::Reservation.count();
            return false;
        };
        if push_bytes.is_some_and(|bytes| !inline_space(mem, device.config().layout, bytes, false)) {
            Ui::Order.count();
            return false;
        }
        let candidate = Direct::new(&self.admission, device.swap_count());
        let mut memory = DeviceMemory(mem);
        let result = if push_bytes.is_some() {
            device.try_native_pushed_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        } else {
            device.try_native_immediate_draw(&mut memory, PrimitiveType::TriangleFan, vertices, &candidate)
        };
        let drawn = matches!(result, Ok(true));
        if !drawn {
            Ui::Reservation.count();
        }
        drawn
    }

    fn permits_draw(&self) -> bool {
        SCOPES.with(|scopes| scopes.borrow().permits(self.admission.owner))
    }

    fn scope(&self, kind: UiScope, stack: u32, frame: u32, eligible: bool) -> Scope {
        Scope { owner: self.admission.owner, kind, stack, frame, eligible }
    }
}

// Begin and End take two words each. Each vertex takes a two-word
// packed colour and two three-word float2 writes.
const SCREEN_FAN_PUSH_BYTES: u32 = (2 + 4 * (2 + 3 + 3) + 2) * 4;

fn inline_space(mem: &dyn GuestMemory, layout: d3d::layout::GuestLayout, bytes: u32, open: bool) -> bool {
    use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

    const BEGIN: u32 = 0x800;
    const FENCE_PENDING: u32 = 0x1000;
    let expected = if open { BEGIN } else { 0 };
    mem.u32(layout.p_device).ok().is_some_and(|device| {
        device == layout.device
            && mem.u32(device + FLAGS).is_ok_and(|flags| flags & (BEGIN | FENCE_PENDING) == expected)
            && mem.u32(device + PUT).ok()
                .and_then(|put| put.checked_add(bytes))
                .is_some_and(|end| mem.u32(device + THRESHOLD).is_ok_and(|limit| end <= limit))
    })
}

impl<B: d3d::Backend + Send + 'static> Host for Glue<B> {
    fn draw_quad(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, None)
    }

    fn screen_fan(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, Some(SCREEN_FAN_PUSH_BYTES))
    }

    fn fade_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        self.fan(mem, vertices, None)
    }

    fn loading_bitmap_fan(&self, mem: &mut dyn GuestMemory, vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        self.fan(mem, vertices, None)
    }

    fn backdrop_wash(&self, mem: &mut dyn GuestMemory, vertices: &HudVertices) -> bool {
        self.fan(mem, vertices, Some(engine::ui_backdrop::WASH_PUSH_BYTES))
    }

    fn backdrop_composite(
        &self,
        mem: &mut dyn GuestMemory,
        vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        self.fan(mem, vertices, None)
    }

    fn model_ranges(&self, mem: &mut dyn GuestMemory, ranges: &[engine::ui_model::Range]) -> bool {
        if !SCOPES.with(|s| {
            s.borrow().active(self.admission.owner)
                .is_some_and(|scope| scope.kind == UiScope::Model && scope.eligible)
        }) {
            return false;
        }
        let Some((head, rest)) = ranges.split_first() else { return false };
        if rest.iter().any(|range| range.strip != head.strip) {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            return false;
        };
        let mut candidate = Direct::new(&self.admission, device.swap_count());
        let mut memory = DeviceMemory(mem);
        if !rest.is_empty() {
            if !matches!(device.drain_admission_boundary(&mut memory), Ok(true)) {
                Ui::Order.count();
                return false;
            }
            let ranges: Vec<_> = rest.iter().map(|r| (r.indices, r.count)).collect();
            let Ok(siblings) = device.indexed_siblings(&memory, &ranges) else {
                Ui::Geometry.count();
                return false;
            };
            candidate.siblings = siblings.into();
        }
        let primitive = if head.strip { PrimitiveType::TriangleStrip } else { PrimitiveType::TriangleList };
        device.try_native_indexed_draw(
            &mut memory, primitive, head.count, head.indices, &candidate,
        ).ok() == Some(true)
    }

    fn outline(&self, mem: &mut dyn GuestMemory, vertices: &OutlineVertices) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Ok(mut device) = self.device.lock() else {
            return false;
        };
        if !inline_space(mem, device.config().layout, OUTLINE_PUSH_BYTES, true) {
            Ui::Order.count();
            return false;
        }
        let candidate = Direct::new(&self.admission, device.swap_count());
        device.try_native_open_pushed_draw(
            &mut DeviceMemory(mem), PrimitiveType::LineStrip, vertices, &candidate,
        ).ok() == Some(true)
    }

    fn inline_ui_quad(&self, mem: &mut dyn GuestMemory, primitive: u32, bytes: Vec<u8>) -> bool {
        if !self.permits_draw() {
            return false;
        }
        let Some(primitive) = PrimitiveType::from_xbox(primitive) else { return false };
        let Ok(mut device) = self.device.lock() else { return false };
        let candidate = Direct::new(&self.admission, device.swap_count());
        let drawn = device.try_native_inline_draw(
            &mut DeviceMemory(mem), primitive, bytes, &[None; 16], &candidate,
        ).ok() == Some(true);
        if !drawn {
            Ui::Reservation.count();
        }
        drawn
    }

    fn enter_ui(&self, mem: &mut dyn GuestMemory, kind: UiScope, stack: u32, eligible: bool) {
        let Ok(mut device) = self.device.lock() else {
            Ui::Order.count();
            return;
        };
        // Work before the callback belongs to its prior producer.
        let ordered = device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() == Some(true);
        if !ordered {
            Ui::Order.count();
        }
        let scope = self.scope(kind, stack, device.swap_count(), ordered && eligible);
        SCOPES.with(|scopes| scopes.borrow_mut().enter(scope));
    }

    fn leave_ui(&self, mem: &mut dyn GuestMemory, kind: UiScope, stack: u32) {
        let scope = self.scope(kind, stack, 0, false);
        let Some(index) = SCOPES.with(|scopes| scopes.borrow().position(scope)) else { return };
        if !self.device.lock().is_ok_and(|mut device| {
            device.drain_admission_boundary(&mut DeviceMemory(mem)).ok() == Some(true)
        }) {
            Ui::Order.count();
        }
        SCOPES.with(|scopes| scopes.borrow_mut().0.truncate(index));
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_hud::on(&switches) {
        return;
    }
    static OWNER: AtomicU64 = AtomicU64::new(0);
    let admission = Arc::new(Admission {
        owner: OWNER.fetch_add(1, Ordering::Relaxed),
        gpu: Arc::new(Mutex::new(UiGpu::default())),
    });
    if let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
    }
    render_bridge::native_hud::set_host(Arc::new(Glue {
        device: device.clone(),
        admission,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn ui_scope_pairs_require_the_same_callback_and_stack() {
        let mut scopes = ScopeStack::default();
        let layers = Scope { owner: 1, kind: UiScope::Layers, stack: 0x2000, frame: 2, eligible: true };
        let widget = Scope { kind: UiScope::Widget, stack: 0x1000, ..layers };
        scopes.enter(layers);
        scopes.enter(widget);
        assert_eq!(scopes.active(1), Some(widget));
        assert_eq!(scopes.active(2), None);
        assert_eq!(scopes.position(Scope { stack: 0x1004, ..widget }), None);
        assert_eq!(scopes.position(Scope { frame: 0, ..widget }), Some(1));
        let nested = Scope { kind: UiScope::Layers, stack: 0x800, eligible: false, ..layers };
        scopes.enter(nested);
        assert!(!scopes.permits(1));
        let child = Scope { stack: 0x700, ..widget };
        scopes.enter(child);
        assert!(!scopes.permits(1));
        let next_widget = Scope { stack: 0x1100, ..widget };
        scopes.enter(next_widget);
        assert_eq!(scopes.position(nested), None);
        assert_eq!(scopes.position(widget), None);
        assert_eq!(scopes.active(1), Some(next_widget));
        assert!(scopes.permits(1));
        scopes.0.truncate(scopes.position(next_widget).unwrap());
        assert_eq!(scopes.active(1), Some(layers));
    }

    #[test]
    fn ui_inline_fan_keeps_the_guest_wrap_or_deferred_fence() {
        use d3d::layout::device_field::{FLAGS, PUT, THRESHOLD};

        let layout = d3d::GuestLayout::HALO2;
        let mut mem = FakeGuest::new();
        let put = 0x8000;
        mem.put_u32(layout.p_device, layout.device)
            .put_u32(layout.device + FLAGS, 0)
            .put_u32(layout.device + PUT, put)
            .put_u32(layout.device + THRESHOLD, put + 144);
        assert!(inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + THRESHOLD, put + 128);
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + THRESHOLD, put + 4096)
            .put_u32(layout.device + FLAGS, 0x1000);
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + FLAGS, 0x0800);
        assert!(inline_space(&mem, layout, OUTLINE_PUSH_BYTES, true));
        assert!(!inline_space(&mem, layout, SCREEN_FAN_PUSH_BYTES, false));
        mem.put_u32(layout.device + FLAGS, 0x1800);
        assert!(!inline_space(&mem, layout, OUTLINE_PUSH_BYTES, true));
    }
}
