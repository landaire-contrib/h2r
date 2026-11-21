//! Model-widget ownership begins at the validated sorted UI item. The guest
//! still updates scenario object flags and configures the view and materials.
use std::cell::Cell;
use std::sync::Arc;

use engine::{memory::GuestMemory, points::ui_render::*, ui_model};
use runtime::{CpuAbi, GuestFault, abi::TransferAbi};
use timeline::native::{Metric, Stage, Ui};

use hooks::{Builder, Call, Flow};

use super::{host, ui};

const VIEW_ACTIVE: u32 = 0x004b_a010;
const MODEL_PARENT_BOUNDS: u32 = 0x7c;
const MODEL_WIDGET_LOCAL: u32 = 0x5c;
const PRIMARY_COUNT: u32 = 0x58;
const SECONDARY_COUNT: u32 = 0x64;
const PRIMARY_HANDLES_BELOW_FRAME: u32 = 0x7c;
const SECONDARY_HANDLES: u32 = 4;
const PRIMARY_CAPACITY: u32 = 32;
const SECONDARY_CAPACITY: u32 = 8;
const VIEW_STACK_BYTES: u32 = 24;
const GEOMETRY_STACK_BYTES: u32 = 12;
const GEOMETRY_LAYER_CALLER: u32 = 0x0003_bc90;
const GEOMETRY_TRANSPARENT_CALLER: u32 = 0x0004_1010;
const GEOMETRY_CALLER_FROM_STACK: u32 = 0x4c;
const TEXTURES_REQUESTED: u32 = 0x0051_f3d8;
const TEXTURES_APPLIED: u32 = 0x0051_f3c8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Widget {
    thread: u32,
    address: u32,
    stack_after: u32,
    view_stack: Option<u32>,
    refused: bool,
}

thread_local! {
    static WIDGET: Cell<Option<Widget>> = const { Cell::new(None) };
}

pub(super) fn try_idle(c: &mut Call<'_>, mem: &dyn GuestMemory, stack: u32) -> Option<TransferAbi> {
    if ui_model::is_idle(mem, c.cpu.ecx) != Some(true) {
        return None;
    }
    if !c.ctx.runtime().read_dispatch(|d| d.has_resume(ITEM_CONTINUATION)) {
        Ui::ModelEntry.count();
        return None;
    }
    Ui::ModelIdle.count();
    Stage::Ui.count(Metric::BuildsSkipped, 1);
    c.cpu.esp = stack;
    Some(TransferAbi::resume(ITEM_CONTINUATION))
}

pub(super) fn begin(c: &Call<'_>, stack_after: u32) {
    WIDGET.set(Some(Widget {
        thread: c.ctx.thread_id,
        address: c.cpu.ecx,
        stack_after,
        view_stack: None,
        refused: false,
    }));
}

pub(super) fn end(c: &Call<'_>) -> bool {
    let Some(widget) = WIDGET.get() else { return false };
    if widget.thread != c.ctx.thread_id || widget.stack_after != c.cpu.esp {
        return false;
    }
    if widget.refused {
        Ui::ModelWidgetFallback.count();
    }
    WIDGET.set(None);
    true
}

fn live(c: &Call<'_>) -> Option<Widget> {
    WIDGET.get().filter(|w| w.thread == c.ctx.thread_id && c.cpu.esp < w.stack_after)
}

fn refuse(mut widget: Widget, reason: Ui) {
    widget.refused = true;
    WIDGET.set(Some(widget));
    reason.count();
}

fn camera_matches(
    mem: &dyn GuestMemory,
    widget: u32,
    frame: u32,
    center: u32,
    stack: u32,
    mxcsr: u32,
) -> Option<bool> {
    let word = |offset| mem.u32(frame.checked_add(offset)?).ok();
    if word(MODEL_WIDGET_LOCAL)? != widget {
        return Some(false);
    }
    let primary = word(PRIMARY_COUNT)?;
    let secondary = word(SECONDARY_COUNT)?;
    if !(1..=PRIMARY_CAPACITY).contains(&primary) || secondary > SECONDARY_CAPACITY {
        return Some(false);
    }
    for (handles, count) in [
        (frame.checked_sub(PRIMARY_HANDLES_BELOW_FRAME)?, primary),
        (frame.checked_add(SECONDARY_HANDLES)?, secondary),
    ] {
        for index in 0..count {
            if mem.u32(handles.checked_add(4 * index)?).ok()? == u32::MAX {
                return Some(false);
            }
        }
    }
    let camera = ui_model::camera(mem, widget, word(MODEL_PARENT_BOUNDS)?, mxcsr)?;
    for (i, component) in camera.center.into_iter().enumerate() {
        if mem.u32(center.checked_add(i as u32 * 4)?).ok()? != component {
            return Some(false);
        }
    }
    for (i, coordinate) in camera.bounds.into_iter().enumerate() {
        if mem.u16(stack.checked_add(i as u32 * 4)?).ok()? != coordinate {
            return Some(false);
        }
    }
    Some(mem.u32(stack.checked_add(20)?).ok()? == camera.fov)
}

fn view(c: &Call<'_>) {
    let Some(mut widget) = live(c) else { return };
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|d| {
        d.has_generated(MODEL_VIEW_RETURN.base) && d.has_generated(MODEL_GEOMETRY_CALL.base)
    }) {
        refuse(widget, Ui::ModelEntry);
        return;
    }
    let mem = rt.memory();
    if camera_matches(&*mem, widget.address, c.cpu.ebp, c.cpu.eax, c.cpu.esp, c.cpu.mxcsr)
        != Some(true)
    {
        refuse(widget, Ui::ModelCamera);
        return;
    }
    widget.view_stack = Some(c.cpu.esp);
    WIDGET.set(Some(widget));
}

fn textures_applied(mem: &dyn GuestMemory) -> bool {
    (0..4).all(|stage| {
        let requested = mem.u32(TEXTURES_REQUESTED + 4 * stage).ok();
        requested.is_some() && requested == mem.u32(TEXTURES_APPLIED + 4 * stage).ok()
    })
}

fn plan(c: &Call<'_>, mem: &dyn GuestMemory, widget: Widget) -> Option<(u32, Vec<ui_model::Range>)> {
    let esp = c.cpu.esp;
    let view = widget.view_stack?;
    // Require both the widget view's call frame and the standard material
    // handler. The same geometry function is shared with the main scene.
    if esp >= view
        || mem.u32(view.checked_sub(4)?).ok() != Some(MODEL_VIEW_CONTINUATION)
        || mem.u8(VIEW_ACTIVE).ok() != Some(1)
    {
        return None;
    }
    let word = |offset| mem.u32(esp.checked_add(offset)?).ok();
    if !matches!(word(GEOMETRY_CALLER_FROM_STACK), Some(GEOMETRY_LAYER_CALLER | GEOMETRY_TRANSPARENT_CALLER)) {
        refuse(widget, Ui::ModelCaller);
        return None;
    }
    Stage::Ui.count(Metric::WalkExamined, 1);
    if !c.ctx.runtime().read_dispatch(|d| d.has_resume(MODEL_GEOMETRY_RETURN)) {
        refuse(widget, Ui::ModelEntry);
        return None;
    }
    let next_stack = esp.checked_add(GEOMETRY_STACK_BYTES)?;
    let masked = c.cpu.eax as u8 != 0;
    let (Some(section), Some(group), Some(mask)) = (word(0), word(4), word(8)) else {
        refuse(widget, Ui::ModelGeometry);
        return None;
    };
    match ui_model::ranges(mem, section, group, masked.then_some(mask), c.cpu.ecx as u8 != 0) {
        Ok(ranges) => Some((next_stack, ranges)),
        Err(reason) => {
            refuse(widget, match reason {
                ui_model::Refusal::Inputs => Ui::ModelGeometry,
                ui_model::Refusal::RangeLimit => Ui::ModelRangeLimit,
            });
            Stage::Ui.count(Metric::WalkRefusedInputs, 1);
            None
        }
    }
}

fn finish(cpu: &mut CpuAbi, next_stack: u32, count: u32) -> TransferAbi {
    cpu.eax = count;
    cpu.esp = next_stack;
    Ui::ModelNativeGroup.count();
    ui::record(true);
    TransferAbi::resume(MODEL_GEOMETRY_RETURN)
}

fn record(mem: &mut dyn GuestMemory, widget: Widget, ranges: &[ui_model::Range]) -> bool {
    let drawn = host().is_some_and(|h| h.model_ranges(mem, ranges));
    if !drawn {
        refuse(widget, Ui::ModelReservation);
        ui::record(false);
    }
    drawn
}

fn geometry(c: &mut Call<'_>) -> Result<Flow, GuestFault> {
    let Some(widget) = live(c) else { return Ok(Flow::Continue) };
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let Some((next_stack, ranges)) = plan(c, &*mem, widget) else {
        return Ok(Flow::Continue);
    };
    if ranges.is_empty() {
        return Ok(Flow::Return(finish(c.cpu, next_stack, 0)));
    }
    let count = ranges.iter().map(|r| r.count).sum();
    if textures_applied(&*mem) {
        return Ok(if record(&mut *mem, widget, &ranges) {
            Flow::Return(finish(c.cpu, next_stack, count))
        } else {
            Flow::Continue
        });
    }
    if !rt.read_dispatch(|d| {
        d.has_generated(MODEL_TEXTURE_COMMIT) && d.has_generated(MODEL_GEOMETRY_EMITTER)
    }) {
        refuse(widget, Ui::ModelTextureCommit);
        return Ok(Flow::Continue);
    }

    // Use the same state commit that the emitter would run. If recording
    // fails, the applied mirror makes the retail emitter's repeat commit a
    // no-op. No stream or range of the replacement has been published.
    let saved = *c.cpu;
    let transfer = c.ctx.call_guest_named(
        c.cpu, &mut mem, MODEL_TEXTURE_COMMIT, "native UI model texture commit",
        move |cpu, ctx| {
            *cpu = saved;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            if WIDGET.get() == Some(widget) {
                if !textures_applied(&*mem) {
                    refuse(widget, Ui::ModelTextureCommit);
                } else if record(&mut *mem, widget, &ranges) {
                    return Ok(finish(cpu, next_stack, count));
                }
            }
            mem.call(cpu, MODEL_GEOMETRY_EMITTER, MODEL_GEOMETRY_RETURN)
        },
    )?;
    Ok(Flow::Return(transfer))
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw()
        .before(&MODEL_VIEW_CALL, "native_ui_model_view", |c| {
            view(c);
            Ok(Flow::Continue)
        })
        .context_only()
        .optional();
    b.raw()
        .before(&MODEL_VIEW_RETURN, "native_ui_model_view_done", |c| {
            if let Some(mut widget) = live(c)
                && widget.view_stack.and_then(|s| s.checked_add(VIEW_STACK_BYTES)) == Some(c.cpu.esp)
            {
                widget.view_stack = None;
                WIDGET.set(Some(widget));
            }
            Ok(Flow::Continue)
        })
        .context_only()
        .optional();
    b.raw()
        .before(&MODEL_GEOMETRY_CALL, "native_ui_model_geometry", geometry)
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn model_builder_only_skips_a_noop_texture_commit() {
        let mut mem = FakeGuest::new();
        assert!(!textures_applied(&mem));
        for i in 0..4u32 {
            mem.put_u32(TEXTURES_REQUESTED + 4 * i, 100 + i)
                .put_u32(TEXTURES_APPLIED + 4 * i, 100 + i);
        }
        assert!(textures_applied(&mem));
        mem.put_u32(TEXTURES_REQUESTED + 8, 500);
        assert!(!textures_applied(&mem));
    }
}
