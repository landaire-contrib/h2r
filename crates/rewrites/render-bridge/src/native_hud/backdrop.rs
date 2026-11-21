//! The three backdrop draws share the guest's material and target setup.
use std::sync::Arc;

use engine::{memory::GuestMemory, points::ui_render::*, ui_backdrop};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage, Ui};

use hooks::{Builder, Call, Flow};

use super::{host, ui};

#[derive(Clone, Copy)]
enum Pass {
    Wash,
    Tint,
    Composite,
}

impl Pass {
    fn tail(self) -> u32 {
        match self {
            Self::Wash => BACKDROP_WASH_TAIL,
            Self::Tint => BACKDROP_TINT_TAIL,
            Self::Composite => BACKDROP_COMPOSITE_TAIL,
        }
    }
}

fn fan_stack(mem: &dyn GuestMemory, esp: u32, pass: Pass) -> Option<u32> {
    match pass {
        Pass::Wash => Some(esp),
        Pass::Tint | Pass::Composite if mem.u32(esp).ok() == Some(7) => esp.checked_add(4),
        _ => None,
    }
}

fn draw(c: &mut Call<'_>, pass: Pass) -> Option<TransferAbi> {
    Stage::Ui.count(Metric::WalkExamined, 1);
    let rt = Arc::clone(c.ctx.runtime());
    if !rt.read_dispatch(|dispatch| dispatch.has_resume(pass.tail())) {
        Ui::BackdropEntry.count();
        return None;
    }
    let mut mem = rt.memory();
    let Some(esp) = fan_stack(&*mem, c.cpu.esp, pass) else {
        return refuse_inputs();
    };
    // All three layouts write UV to register 1. Wash starts in the middle
    // of the inline branch, before any of its push words or flags change.
    if c.cpu.ebx != 1 || (!matches!(pass, Pass::Composite) && c.cpu.ebp != 0) {
        return refuse_inputs();
    }
    let mut mxcsr = c.cpu.mxcsr;
    let Some(host) = host() else {
        Ui::BackdropReservation.count();
        ui::record(false);
        return None;
    };
    let drawn = match pass {
        Pass::Wash => {
            let Some(vertices) = ui_backdrop::wash(&*mem, esp, c.cpu.edi, &mut mxcsr) else {
                return refuse_inputs();
            };
            host.backdrop_wash(&mut *mem, &vertices)
        }
        Pass::Tint => {
            let Some(vertices) = ui_backdrop::tint(&*mem) else {
                return refuse_inputs();
            };
            host.draw_quad(&mut *mem, &vertices)
        }
        Pass::Composite => {
            let Some(vertices) = ui_backdrop::composite(&*mem, esp, &mut mxcsr) else {
                return refuse_inputs();
            };
            host.backdrop_composite(&mut *mem, &vertices)
        }
    };
    ui::record(drawn);
    if !drawn {
        Ui::BackdropReservation.count();
        return None;
    }
    c.cpu.esp = esp;
    c.cpu.mxcsr = mxcsr;
    // End leaves the position register selector in EDI. The wash and
    // composite epilogues immediately overwrite it.
    c.cpu.edi = u32::MAX;
    Ui::BackdropNative.count();
    Some(TransferAbi::resume(pass.tail()))
}

fn refuse_inputs() -> Option<TransferAbi> {
    Ui::BackdropInputs.count();
    Stage::Ui.count(Metric::WalkRefusedInputs, 1);
    None
}

pub(super) fn place(b: &mut Builder<'_>) {
    for (site, pass) in [
        (&BACKDROP_WASH, Pass::Wash),
        (&BACKDROP_TINT, Pass::Tint),
        (&BACKDROP_COMPOSITE, Pass::Composite),
    ] {
        b.raw()
            .before(site, "native_ui_backdrop_fan", move |c| {
                Ok(draw(c, pass).map_or(Flow::Continue, Flow::Return))
            })
            .context_only()
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn api_fans_require_the_guest_begin_argument() {
        let mut mem = FakeGuest::new();
        let esp = 0x1000;
        mem.put_u32(esp, 6);
        assert_eq!(fan_stack(&mem, esp, Pass::Tint), None);
        assert_eq!(fan_stack(&mem, esp, Pass::Wash), Some(esp));
        mem.put_u32(esp, 7);
        assert_eq!(fan_stack(&mem, esp, Pass::Tint), Some(esp + 4));
        assert_eq!(fan_stack(&mem, esp, Pass::Composite), Some(esp + 4));
    }
}
