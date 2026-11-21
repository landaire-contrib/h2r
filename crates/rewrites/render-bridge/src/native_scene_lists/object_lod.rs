//! Model section selection by camera distance, and the projected-size byte
//! carried to material draws. The object setup's original pixel radius also
//! schedules lighting samples read by effect updates on the game tick, so only
//! these final consumers apply the view's factor.

use super::*;

const MODEL_LOD_RETURN: u32 = 0x0004_be52;
const MODEL_DISTANCES: u32 = 0x28;
const POSE_SECTIONS: u32 = 0xc4;
const CLUSTER_RECORD_RETURN: u32 = 0x0013_3509;
const PLACEMENT_RECORD_RETURN: u32 = 0x0013_36d8;

/// More output lines keep detail farther away: the model picks the detail it
/// would have `scale` times nearer.
///
/// At 0xbd750 the selected section bytes index this bitset. A hit can clear
/// the dirty bit in the live object and evaluate its pose. Keep the choice
/// and its per-view hysteresis retail for these models, even on clean frames
/// and for attached objects. Section 0xff denotes an absent region.
pub(super) fn model_distance(m: &dyn GuestMemory, hlmt: u32, distance: f32, scale: f32) -> f32 {
    if scale == 1.0 || !(scale.is_finite() && scale > 0.0 && distance.is_finite()) {
        return distance;
    }
    let Some(mask) = hlmt.checked_add(POSE_SECTIONS) else {
        return distance;
    };
    let mut sections = [0u8; 32];
    if m.read(mask, &mut sections).is_err() {
        return distance;
    }
    sections[31] &= 0x7f;
    if sections == [0; 32] {
        (distance / scale).clamp(-f32::MAX, f32::MAX)
    } else {
        distance
    }
}

/// The writer floors pixels * unit and saturates to a byte. Bound large
/// scaled values before its guest float-to-i32 conversion can overflow.
pub(super) fn record_pixels(pixels: f32, scale: f32, unit: f32) -> f32 {
    if scale == 1.0 || !(scale.is_finite() && scale > 0.0) {
        return pixels;
    }
    bound_for_writer(pixels * scale, unit)
}

fn bound_for_writer(pixels: f32, unit: f32) -> f32 {
    // Within signed i32 range the guest already clamps correctly. Outside
    // it cvttss2si produces INT_MIN, which would turn a huge radius into 0.
    if !(unit > 0.0 && unit.is_finite()) {
        return pixels;
    }
    let quantized = pixels * unit;
    if quantized >= i32::MAX as f32 {
        (256.0 / unit).min(f32::MAX)
    } else if quantized < i32::MIN as f32 {
        0.0
    } else {
        pixels
    }
}

fn guest_record_argument(m: &dyn GuestMemory, esp: u32, factor: f32) -> Option<(u32, f32)> {
    if factor == 1.0 {
        return None;
    }
    let caller = m.u32(esp).ok()?;
    let arg = esp.checked_add(4 + 0x24)?;
    let pixels = m.f32(arg).ok()?;
    let unit = m.f32(records::PIXEL_QUANTIZE).ok()?;
    let scaled = match caller {
        points::OBJECT_RECORD_APPENDED => record_pixels(pixels, factor, unit),
        // Structure callers already use the scaled projection. Only guard
        // their conversion when the guest handles the writer itself.
        CLUSTER_RECORD_RETURN | PLACEMENT_RECORD_RETURN => bound_for_writer(pixels, unit),
        _ => return None,
    };
    (scaled.to_bits() != pixels.to_bits()).then_some((arg, scaled))
}

pub(super) fn install(b: &mut Builder, scale: ViewLodScale) {
    b.raw()
        .before(&points::MODEL_LOD, "scale model LOD distance", move |c| {
            if c.addr != points::MODEL_LOD.base {
                return Ok(Flow::Continue);
            }
            let mem = c.ctx.runtime().memory();
            let factor = scale.read(&*mem);
            if factor != 1.0 && mem.u32(c.cpu.esp).ok() == Some(MODEL_LOD_RETURN)
                && let Some(hlmt) = c.cpu.ecx.checked_sub(MODEL_DISTANCES)
            {
                let lane = &mut c.cpu.xmm[4][..4];
                let raw = f32::from_le_bytes(lane[..].try_into().unwrap());
                lane.copy_from_slice(&model_distance(&*mem, hlmt, raw, factor).to_le_bytes());
            }
            Ok(Flow::Continue)
        })
        .context_only();

    b.raw()
        .before(
            &points::SCENE_RECORD_WRITER,
            "bound projected record size",
            move |c| {
                if c.addr != points::SCENE_RECORD_WRITER.base {
                    return Ok(Flow::Continue);
                }
                let mut mem = c.ctx.runtime().memory();
                let factor = scale.read(&*mem);
                if let Some((arg, scaled)) = guest_record_argument(&*mem, c.cpu.esp, factor) {
                    mem.write_bytes_from(c.addr, arg, &scaled.to_le_bytes(), 1)?;
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn a_model_with_pose_dependent_sections_keeps_retail_hysteresis_inputs() {
        let hlmt = 0x10000;
        let mut m = FlatMemory::new(hlmt, 0x100);
        assert_eq!(model_distance(&m, hlmt, 28.0, 1.0), 28.0);
        assert_eq!(model_distance(&m, hlmt, 28.0, 4.0), 7.0);
        assert_eq!(model_distance(&m, hlmt, 21.0, 0.5), 42.0);
        m.set_u8(hlmt + POSE_SECTIONS + 31, 0x40).unwrap();
        assert_eq!(model_distance(&m, hlmt, 28.0, 4.0), 28.0);
        m.set_u8(hlmt + POSE_SECTIONS + 31, 0x80).unwrap();
        assert_eq!(model_distance(&m, hlmt, 28.0, 4.0), 7.0);
    }

    #[test]
    fn guest_record_callers_scale_their_radius_once_and_bound_conversion() {
        use engine::testing::FakeGuest;
        let esp = 0x10000;
        let arg = esp + 0x28;
        let mut m = FakeGuest::new();
        m.put_f32(records::PIXEL_QUANTIZE, 0.5);
        for caller in [CLUSTER_RECORD_RETURN, PLACEMENT_RECORD_RETURN] {
            m.put_u32(esp, caller).put_f32(arg, 7.0);
            assert_eq!(guest_record_argument(&m, esp, 4.0), None);
            m.put_f32(arg, f32::INFINITY);
            assert_eq!(guest_record_argument(&m, esp, 4.0), Some((arg, 512.0)));
            assert_eq!(guest_record_argument(&m, esp, 1.0), None);
            m.put_f32(arg, f32::NEG_INFINITY);
            assert_eq!(guest_record_argument(&m, esp, 4.0), Some((arg, 0.0)));
        }
        m.put_u32(esp, points::OBJECT_RECORD_APPENDED)
            .put_f32(arg, 7.0);
        assert_eq!(guest_record_argument(&m, esp, 4.0), Some((arg, 28.0)));
        m.put_u32(esp, 0x20000);
        assert_eq!(guest_record_argument(&m, esp, 4.0), None);
    }

    #[test]
    fn projected_record_bytes_scale_before_flooring_and_saturate() {
        let byte = |pixels, scale| {
            (record_pixels(pixels, scale, 0.5) * 0.5)
                .floor()
                .clamp(0., 255.) as u8
        };
        assert_eq!(byte(2.9, 1.0), 1);
        assert_eq!(byte(2.9, 4.0), 5);
        assert_eq!(byte(2.9, 4.0 * 1.5), 8);
        assert_eq!(byte(-10.0, 4.0), 0);
        assert_eq!(byte(f32::MAX, 24.0), 255);
        assert_eq!(byte(f32::INFINITY, 4.0), 255);
    }
}
