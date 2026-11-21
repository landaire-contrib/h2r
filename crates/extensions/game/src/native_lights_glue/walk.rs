//! Resolve the first standard handler's live bindings once for the entire
//! gathered group. The cursor write and GPU submission share the device's
//! synchronous reservation. Refusal draws the original leader with its
//! untouched suffix.

use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalDraws, ExternalPassRequest, IndexedStreams, IndexedTail,
};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalWrites};
use render_bridge::native_lights::walk::{self, Batch};
use native_render::world_layers::DRAW_INDEXED_VERTICES;
use native_render::world_layers_gpu::{Cache, IndexedPass};
use timeline::native::Stage;

pub fn install<B: d3d::Backend + 'static>(
    rt: &runtime::Runtime,
    device: &d3d::abi::SharedDevice<B>,
    shadows: Option<Arc<dyn DrawAdmission>>,
) {
    if !walk::on(&hooks::Switches::from_env().unwrap_or_default()) {
        return;
    }
    let device = Arc::clone(device);
    let cache = Arc::new(Cache::default());
    rt.edit_dispatch(|dispatch| {
        let Some(previous) = dispatch.override_handler(DRAW_INDEXED_VERTICES) else { return };
        let safe = dispatch.is_context_only(DRAW_INDEXED_VERTICES);
        dispatch.register_override(
            DRAW_INDEXED_VERTICES,
            "boot: gathered light geometry",
            move |cpu, ctx, selector| {
                let scoped = if walk::waiting() {
                    begin_batch(&mut ctx.memory(), cpu.esp, &device, &cache, &shadows)
                } else {
                    None
                };
                let result = previous.invoke(cpu, ctx, selector);
                if let Some((scope, batch)) = scoped && scope.committed() {
                    walk::committed(batch.layer, batch.slots as usize);
                }
                Ok(result)
            },
        );
        if safe {
            dispatch.mark_context_only(DRAW_INDEXED_VERTICES);
        }
    });
}

fn begin_batch<B: d3d::Backend>(
    mem: &mut runtime::Guest,
    esp: u32,
    device: &d3d::abi::SharedDevice<B>,
    cache: &Arc<Cache>,
    shadows: &Option<Arc<dyn DrawAdmission>>,
) -> Option<(d3d::DrawScope, Batch)> {
    let batch = walk::take_batch(mem, esp)?;
    let stage = if matches!(batch.layer, 10 | 11 | 14) {
        shadows.as_ref()?;
        Stage::Shadows
    } else {
        Stage::Lights
    };
    let tail = IndexedTail {
        indexed_esp: esp,
        counter: batch.counter,
        remaining: batch.slots,
    };
    let admission = Arc::new(Admission {
        view: batch.view,
        stage,
        tail,
        first_count: batch.ranges[0].1,
        siblings: Mutex::new(None),
        shadow: shadows.clone(),
        cache: Arc::clone(cache),
        frame: crate::d3d_diag::presents() + 1,
    });
    let mut device = device.lock().ok()?;
    let mut mem = super::DeviceMemory(mem);
    let scope = device.begin_draw_scope(&mut mem, admission.clone()).ok()??;
    let siblings = device.indexed_siblings(&mem, &batch.ranges[1..]).ok()?;
    *admission.siblings.lock().ok()? = Some(siblings);
    Some((scope, batch))
}

struct Admission {
    stage: Stage,
    view: u32,
    tail: IndexedTail,
    first_count: u32,
    siblings: Mutex<Option<Vec<IndexedStreams>>>,
    shadow: Option<Arc<dyn DrawAdmission>>,
    cache: Arc<Cache>,
    frame: u64,
}

impl Admission {
    fn range(&self, snapshot: &DrawSnapshot<'_>) -> Option<(usize, Box<dyn ExternalPass>)> {
        if self.stage == Stage::Shadows {
            let (plan, pass) = self.shadow.as_ref()?.admit(snapshot)?;
            if !plan.slots.is_empty() || plan.view != self.view { return None; }
            Some((plan.ranges, pass))
        } else {
            let pass = IndexedPass::new(snapshot.draw, Arc::clone(&self.cache), self.frame, None)?;
            Some((1, Box::new(pass)))
        }
    }
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let leader = snapshot.draw;
        if leader.entry != "D3DDevice_DrawIndexedVertices" || leader.hw.zpass_count_enable
            || !leader.state.bad.is_empty() || leader.indices.as_ref()?.len() != self.first_count as usize
            || snapshot.sampled_stages.iter().enumerate().any(|(n, &used)| used &&
                [snapshot.color_header, snapshot.depth_header].contains(&snapshot.bound_textures[n])) {
            return None;
        }
        let siblings = self.siblings.lock().ok()?.take()?;
        let (mut ranges, first) = self.range(snapshot)?;
        let writes = first.writes();
        let colour_only = first.colour_only();
        let mut passes = vec![(ranges, first)];
        let mut draw = leader.clone();
        for next in siblings {
            if next.layout != draw.layout { return None; }
            draw.streams = next.streams;
            draw.indices = Some(next.indices);
            draw.vertex_count = next.vertex_count;
            let sibling = DrawSnapshot { draw: &draw, ..*snapshot };
            let (count, pass) = self.range(&sibling)?;
            if pass.writes() != writes || pass.colour_only() != colour_only { return None; }
            ranges = ranges.checked_add(count)?;
            passes.push((count, pass));
        }
        Some((ExternalPlan { stage: self.stage, view: self.view, ranges, slots: Vec::new() },
            Box::new(Pass { passes, writes, colour_only, tail: self.tail })))
    }
}

struct Pass {
    passes: Vec<(usize, Box<dyn ExternalPass>)>,
    writes: ExternalWrites,
    colour_only: bool,
    tail: IndexedTail,
}

impl ExternalPass for Pass {
    fn indexed_tail(&self) -> Option<IndexedTail> { Some(self.tail) }
    fn writes(&self) -> ExternalWrites { self.writes }
    fn colour_only(&self) -> bool { self.colour_only }

    fn record_draws(&mut self, target: &ExternalAttachments, request: ExternalPassRequest)
        -> Option<Result<ExternalDraws, &'static str>> {
        let mut group = ExternalDraws { packets: Vec::new(), ranges: 0, discards: false, writes: Vec::new() };
        for (expected, pass) in &mut self.passes {
            match pass.record_draws(target, request) {
                Some(Ok(draws)) if draws.ranges == *expected && draws.packets.len() >= *expected => {
                    group.ranges += draws.ranges;
                    group.packets.extend(draws.packets);
                    group.discards |= draws.discards;
                }
                _ => return Some(Err("incomplete light geometry on this target copy")),
            }
        }
        Some(Ok(group))
    }
}
