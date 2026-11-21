//! Whole-call replacement for object render setup. Keep render and object
//! cache writes staged until the last child has been visited.

mod lighting;
mod palette;
mod residency;
mod sections;
mod staged;

use records::ObjectRecord;
use sections::Prepared;
use staged::Staged;

use super::lights::{MIN_DEPTH, PIXEL_SCALE, PROJECTION_DEPTH, SIZE_FACTOR};
use super::objects::{LIGHT_VIS, List, MAIN_VIS, VIEW_KIND, VIS_FLAGS};
use super::*;

const CAMERA: u32 = 0x004b_9da0;
const CAMERA_DISTANCE_SCALE: u32 = CAMERA + 0x2c;
const OBJECT_SIZE_SCALE: u32 = 0x004b_a028;
const FORCE_FULL_LOD: u32 = 0x004b_a019;
const FADE_SPAN_SCALE: u32 = 0x0045_dda8;
const FADE_MIN: u32 = 0x0045_df60;
const DEFAULT_MODEL_DISTANCE: u32 = 0x0044_aed4;
const ONE: u32 = 0x0045_dbc0;
const BYTE_SCALE: u32 = 0x0045_dc38;
const VIEW_CATEGORY: u32 = 0x004b_9ef0;
const VIEW_SKY_DATUM: u32 = 0x004b_9f8c;
const SKY_FADE_FRACTION: u32 = 0x004b_9ffc;
const SKY_FLAT_AMBIENT: u32 = 0x004b_a004;
const DISTANCE_EPSILON: u32 = 0x0045_dbd8;
const MAX_CHILD_DEPTH: usize = 64;
const CAMERA_FORWARD: u32 = CAMERA + 0xc;
const VIS_CAMERA_POSITION: u32 = 0x2a68;

#[derive(Clone, Copy)]
struct Scene {
    tags: u32,
    objects: Pool,
    frame: u32,
    vis: u32,
    kind: u32,
    retry: bool,
    lod_scale: f32,
}

#[derive(Clone, Copy)]
struct ObjectState {
    centre: [f32; 3],
    radius: f32,
    pixels: f32,
    /// Scaled camera distance, the model chooser's input (`0x0004bcc0`'s
    /// second argument). Children keep their root's, as the guest does.
    distance: f32,
    opacity: u8,
    lock_pose: bool,
    flags: u32,
    mode: u32,
    clip_mask: u16,
    group: u32,
    group_cookie: u32,
    group_counter: u32,
    attachment: u32,
    mirror_alpha: f32,
    effect_alpha: f32,
    group_depth: f32,
    mirror_mode: bool,
}

/// The two LOD fades as seen at the section builder's caller. Keep the
/// original comparison order around the clamp and near/far transitions.
fn fades(
    distance: f32,
    radius_span: f32,
    range_end: f32,
    minimum: f32,
    near: f32,
    near_start: f32,
    one: f32,
) -> (f32, f32) {
    let detail = if distance >= range_end + radius_span {
        0.0
    } else if distance > range_end {
        let factor = one - (distance - range_end) / radius_span;
        if factor < 0.0 {
            0.0
        } else if factor > one {
            one
        } else if minimum >= factor {
            0.0
        } else {
            factor
        }
    } else {
        one
    };
    let visible = if near <= 0.0 || distance <= near_start {
        one
    } else if distance >= near {
        0.0
    } else {
        one - (distance - near_start) / (near - near_start)
    };
    (visible, detail)
}

fn lod_bytes(
    m: &dyn GuestMemory,
    table: u32,
    handle: u32,
    body: u32,
    distance: f32,
    radius: f32,
) -> Result<(u8, u8)> {
    if m.u8(FORCE_FULL_LOD)? != 0 || m.u32(at(body, 4)?)? & (1 << 19) != 0 {
        return Ok((255, 255));
    }
    let object = tag(m, table, m.u32(body)?)?;
    let model = m.u32(at(object, 0x38)?)?;
    if model == NONE {
        return Ok((0, 0));
    }
    let root = lights::root_owner(m, handle)?;
    let model = if root != handle {
        let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
        let root_body = objects.body(m, root)?;
        let root_object = tag(m, table, m.u32(root_body)?)?;
        let root_model = m.u32(at(root_object, 0x38)?)?;
        if root_model == NONE {
            model
        } else {
            root_model
        }
    } else {
        model
    };
    let distances = at(tag(m, table, model)?, 0x28)?;
    let range = (4i32 - i32::from(m.u16(at(distances, 0x24)?)?)).clamp(0, 4);
    let range_end = if range == 0 {
        m.f32(DEFAULT_MODEL_DISTANCE)?
    } else {
        m.f32(at(distances, 0xc + range as u32 * 4)?)?
    };
    let near = m.f32(distances)?;
    let near_start = m.f32(at(distances, 4)?)?;
    let span = radius * m.f32(FADE_SPAN_SCALE)?;
    let min = m.f32(FADE_MIN)?;
    let one = m.f32(ONE)?;
    let scale = m.f32(BYTE_SCALE)?;
    if ![range_end, near, near_start, span, min, one, scale]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }
    let (visible, detail) = fades(distance, span, range_end, min, near, near_start, one);
    let byte = |v: f32| -> Result<u8> {
        // The CRT helper truncates the x87 product to an integer.
        let scaled = f64::from(v) * f64::from(scale);
        if !(0.0..=255.0).contains(&scaled) {
            return Err(Refusal::Unsupported);
        }
        Ok(scaled as u8)
    };
    Ok((byte(visible)?, byte(detail)?))
}

fn section_flags(m: &dyn GuestMemory, vis: u32, tags: u32, flags: u16) -> Result<u32> {
    let flags = u32::from(flags);
    let mut section = ((((flags >> 2) & 0x2c00) | (flags & 0x4000)) >> 9) | ((flags & 0x200) << 5);
    if m.u8(at(vis, VIS_FLAGS)?)? & 0x80 != 0 {
        section |= 1;
    }
    if m.u32(VIEW_CATEGORY)? == 3 {
        let sky = m.u32(VIEW_SKY_DATUM)?;
        if sky != NONE
            && flags & 0x400 == 0
            && m.f32(SKY_FADE_FRACTION)? == m.f32(DISTANCE_EPSILON)?
            && m.u8(tag(m, tags, sky)?)? & 2 == 0
        {
            section |= 0x100;
        }
        if m.u8(SKY_FLAT_AMBIENT)? == 0 || flags & 0x400 != 0 {
            section |= 0x80;
        }
    }
    Ok(section)
}

pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi, scale: f32) -> (SceneList, Result<Output>) {
    (SceneList::ObjectSetup, setup(m, cpu, scale))
}

fn setup(m: &dyn GuestMemory, cpu: &CpuAbi, lod_scale: f32) -> Result<Output> {
    fp_supported(cpu)?;
    if cpu.x87.control != 0x027f || cpu.x87.tag != 0xffff {
        return Err(Refusal::Unsupported);
    }
    let vis = cpu.esi;
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }
    let list_addr = ptr(m, at(vis, 0x14)?)?;
    let list = List::read(m, list_addr)?;
    let index = u32::from(stack(m, cpu, 4)? as u16);
    if index as usize >= list.count {
        return Err(Refusal::Layout);
    }
    let handle = m.u32(at(list.column(m, 0xc)?, index * 4)?)?;
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
    let body = objects.body(m, handle)?;
    let [px, py, pz] = m.vec3(at(body, 0x30)?)?;
    let radius = m.f32(at(body, 0x3c)?)?;
    let [x, y, z, w] = PROJECTION_DEPTH;
    let [x, y, z, w] = [m.f32(x)?, m.f32(y)?, m.f32(z)?, m.f32(w)?];
    let depth = (((z * pz + y * py) + x * px) + w).abs();
    let min_depth = m.f32(MIN_DEPTH)?;
    let depth = if depth > min_depth { depth } else { min_depth };
    let pixel_size = (((m.f32(PIXEL_SCALE)? / depth) * radius) * m.f32(SIZE_FACTOR)?)
        * m.f32(OBJECT_SIZE_SCALE)?;
    let [cx, cy, cz] = m.vec3(CAMERA)?;
    let (dx, dy, dz) = (
        f64::from(px) - f64::from(cx),
        f64::from(py) - f64::from(cy),
        f64::from(pz) - f64::from(cz),
    );
    let distance =
        (((dz * dz + dy * dy) + dx * dx).sqrt() * f64::from(m.f32(CAMERA_DISTANCE_SCALE)?)) as f32;
    if ![px, py, pz, radius, depth, pixel_size, distance]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }

    let tags = ptr(m, TAGS)?;
    let (opacity, _) = lod_bytes(m, tags, handle, body, distance, radius)?;
    let row_flags = m.u16(at(list.column(m, 8)?, index * 2)?)?;
    let flags = section_flags(m, vis, tags, row_flags)?;
    let mask = if row_flags & 0x800 == 0 {
        m.u16(at(list.column(m, 0x14)?, index * 2)?)?
    } else {
        u16::MAX
    };
    let explicit_retry = stack(m, cpu, 8)? as u8 != 0;
    let scene = Scene {
        tags,
        lod_scale,
        objects,
        frame: m.u32(FRAME)?,
        vis,
        kind,
        retry: kind == 2 || explicit_retry,
    };
    let mode = if mask == u16::MAX { 1 } else { 0x40 };
    let mut state = ObjectState {
        centre: [px, py, pz],
        radius,
        pixels: pixel_size,
        distance,
        opacity,
        lock_pose: row_flags & 0x200 != 0,
        flags,
        mode: mode | 0x208,
        clip_mask: mask,
        group: stack(m, cpu, 16)?,
        group_cookie: m.u32(cpu.esp.checked_sub(0x20).ok_or(Refusal::Layout)?)?,
        group_counter: stack(m, cpu, 12)?,
        attachment: NONE,
        mirror_alpha: 0.0,
        effect_alpha: 0.0,
        group_depth: 0.0,
        mirror_mode: false,
    };
    let mut staged = Staged::new(m);
    let mut prepared = sections::prepare(&mut staged, scene, handle, state)?;
    if explicit_retry {
        prepared.first_person = false;
        state.flags = (state.flags & !2) | 4;
    } else if prepared.first_person && kind == 1 {
        state.flags &= !4;
    }
    let mut tail_registers = None;
    if opacity == 0 {
        let column = list.column(m, 8)?;
        let address = mark_no_draw(&mut staged, column, index)?;
        tail_registers = Some((address, column));
    } else if prepared.effect_flags & 2 == 0 || kind == 0 {
        let descend = stack(m, cpu, 20)? as u8 != 0;
        Walk {
            scene,
            ancestors: Vec::new(),
        }
        .visit(&mut staged, handle, &prepared, state, descend, false)?;
    }
    let mut out = staged.finish();
    if let Some((ecx, edx)) = tail_registers {
        out.ecx = Some(ecx);
        out.edx = Some(edx);
    }
    out.al = Some(u8::from(prepared.first_person));
    Ok(out)
}

fn mark_no_draw(m: &mut Staged<'_>, column: u32, index: u32) -> Result<u32> {
    let address = at(column, index * 2)?;
    let flags = m.u8(address)?;
    m.set_u8(address, flags | 1)?;
    Ok(address)
}

struct Walk {
    scene: Scene,
    ancestors: Vec<u32>,
}

impl Walk {
    fn visit(
        &mut self,
        m: &mut Staged<'_>,
        handle: u32,
        prepared: &Prepared,
        mut state: ObjectState,
        descend: bool,
        overlay_only: bool,
    ) -> Result<()> {
        let mut emit = true;
        if !prepared.models.is_empty() && !overlay_only {
            let mut group_set = false;
            if let Some(overlay) = &prepared.function_overlay {
                state.group_depth = self.depth(m, state.centre)?;
                Self::allocate_group(m, &mut state, 3)?;
                group_set = true;
                let pass = ObjectState {
                    flags: state.mode,
                    mode: 0,
                    attachment: overlay.attachment,
                    effect_alpha: overlay.alpha,
                    ..state
                };
                self.visit(m, handle, prepared, pass, true, true)?;
            }
            if let Some(overlay) = &prepared.overlay {
                if !group_set {
                    Self::allocate_group(m, &mut state, 1)?;
                }
                // Camo draws instead of the base record. Both passes
                // independently traverse attachments.
                let pass = ObjectState {
                    flags: state.mode,
                    mode: 0,
                    attachment: overlay.attachment,
                    mirror_alpha: overlay.alpha,
                    mirror_mode: true,
                    group_depth: self.depth(m, state.centre)?,
                    ..state
                };
                self.visit(m, handle, prepared, pass, true, true)?;
                emit = false;
            }
            for overlay in prepared.variant_overlays.iter().flatten() {
                let pass = ObjectState {
                    flags: state.flags | 0x8000,
                    attachment: overlay.attachment,
                    effect_alpha: overlay.alpha,
                    ..state
                };
                self.visit(m, handle, prepared, pass, false, true)?;
            }
        }
        if emit {
            self.emit(m, prepared, state, descend)?;
        }
        if !descend || prepared.first_person {
            return Ok(());
        }
        if self.ancestors.len() == MAX_CHILD_DEPTH || self.ancestors.contains(&handle) {
            return Err(Refusal::Layout);
        }
        self.ancestors.push(handle);
        let body = self.scene.objects.body(m, handle)?;
        let mut child = m.u32(at(body, 0x10)?)?;
        for _ in 0..self.scene.objects.end {
            if child == NONE {
                break;
            }
            if self.ancestors.contains(&child) {
                return Err(Refusal::Layout);
            }
            let mut child_state = ObjectState {
                clip_mask: u16::MAX,
                mode: (state.mode & !0x40) | 1,
                ..state
            };
            self.split_weapon(m, child, &mut child_state)?;
            let child_setup = sections::prepare(m, self.scene, child, child_state)?;
            self.visit(m, child, &child_setup, child_state, true, overlay_only)?;
            let body = self.scene.objects.body(m, child)?;
            child = m.u32(at(body, 0xc)?)?;
        }
        self.ancestors.pop();
        if child != NONE {
            return Err(Refusal::Layout);
        }
        Ok(())
    }

    fn depth(&self, m: &dyn GuestMemory, centre: [f32; 3]) -> Result<f32> {
        let eye = m.vec3(at(self.scene.vis, VIS_CAMERA_POSITION)?)?;
        let forward = m.vec3(CAMERA_FORWARD)?;
        let x = (centre[0] - eye[0]) * forward[0];
        let y = (centre[1] - eye[1]) * forward[1];
        let z = (centre[2] - eye[2]) * forward[2];
        let depth = (z + y) + x;
        if depth.is_finite() {
            Ok(depth)
        } else {
            Err(Refusal::ObjectModel)
        }
    }

    fn allocate_group(m: &mut Staged<'_>, state: &mut ObjectState, cookie: u32) -> Result<()> {
        let next = m.u32(state.group_counter)?;
        m.set_u32(state.group_counter, next.wrapping_add(1))?;
        state.group = next;
        state.group_cookie = cookie;
        Ok(())
    }

    fn emit(
        &self,
        m: &mut Staged<'_>,
        prepared: &Prepared,
        state: ObjectState,
        descend: bool,
    ) -> Result<()> {
        let mut first_owner = None;
        for model in &prepared.models {
            let mut record_state = state;
            let owner = m.u32(model.lighting)?;
            if !descend && *first_owner.get_or_insert(owner) != owner {
                continue;
            }
            self.split_weapon(m, owner, &mut record_state)?;
            for (region, &section) in model.sections.iter().enumerate() {
                let record = ObjectRecord {
                    section: if section == u8::MAX {
                        NONE
                    } else {
                        u32::from(section)
                    },
                    selector: model.selector,
                    node_count: model.node_count,
                    flags: record_state.flags
                        | record_state.mode
                        | model.draw_flags
                        | (u32::from(prepared.first_person) << 10),
                    model: model.datum,
                    palette: model.palette,
                    region: region as u32,
                    lighting: model.lighting,
                    clip_mask: record_state.clip_mask,
                    opacity: record_state.opacity,
                    attachment: record_state.attachment,
                    pixels: record_state.pixels,
                    group: record_state.group,
                    group_cookie: record_state.group_cookie,
                    clip_flags: model.attributes[region],
                    full_rect: prepared.effect_flags & 0xa == 0xa,
                    empty_rect: prepared.effect_flags & 0x12 == 0x12,
                    mirror_mode: record_state.mirror_mode,
                    mirror_alpha: record_state.mirror_alpha,
                    effect_alpha: record_state.effect_alpha,
                    group_depth: record_state.group_depth,
                    sphere: (record_state.centre, record_state.radius),
                };
                let writes = record.plan(m, self.scene.lod_scale)?;
                m.apply(writes)?;
            }
        }
        Ok(())
    }

    fn split_weapon(
        &self,
        m: &dyn GuestMemory,
        handle: u32,
        state: &mut ObjectState,
    ) -> Result<()> {
        if state.group_cookie == 1 {
            let body = self.scene.objects.body(m, handle)?;
            if m.u8(at(body, 0xaa)?)? == 2 {
                let def = tag(m, self.scene.tags, m.u32(body)?)?;
                if m.u16(at(def, 0x290)?)? != 0 {
                    state.group = 31;
                    state.group_cookie = NONE;
                    state.attachment = NONE;
                    state.mirror_mode = false;
                    state.mirror_alpha = 0.0;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_setup_lod_uses_both_fade_ranges() {
        let pick = |distance| fades(distance, 10.0, 50.0, 0.001, 20.0, 10.0, 1.0);
        assert_eq!(pick(15.0), (0.5, 1.0));
        assert_eq!(pick(25.0), (0.0, 1.0));
        assert_eq!(pick(55.0), (0.0, 0.5));
        assert_eq!(pick(60.0), (0.0, 0.0));
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use engine::testing::FakeGuest;
    use native_render::object_nodes::{BLOCK_MATRICES, MATRIX_STRIDE, NODE_BLOCKS};
    use native_render::skip::{RENDER_RECORD_SIZE, RENDER_RECORDS};

    const LIST: u32 = 0x10000;
    const FLAGS: u32 = 0x10080;
    const OBJECT_POOL: u32 = 0x11000;
    const OBJECT_SLOTS: u32 = 0x11100;
    const ROOT: u32 = 0x12000;
    const CHILD: u32 = 0x13000;
    const ROOT_HANDLE: u32 = 0x7100_0000;
    const CHILD_HANDLE: u32 = 0x7100_0001;
    const TAG_TABLE: u32 = 0x14000;
    const DEFINITION: u32 = 0x15000;
    const HLMT: u32 = 0x16000;
    const MODEL: u32 = 0x17000;
    const SECTIONS: u32 = 0x18000;
    const LIGHT_CACHE: u32 = 0x19000;
    const CACHE_ENTRIES: u32 = LIGHT_CACHE + 0x100;
    const RESOURCE_CACHE: u32 = 0x1a000;
    const RESOURCE_POOL: u32 = 0x1a100;
    const RESOURCE_SLOTS: u32 = 0x1a200;
    const STACK: u32 = 0x1b100;
    const CURRENT_VIS: u32 = 0x004c_0b80;
    const RECORD_COUNT: u32 = 0x004c_0b78;
    const MAIN_COUNT: u32 = 0x004c_0b7c;

    fn fixture() -> (FakeGuest, CpuAbi) {
        let mut mem = FakeGuest::new();
        mem.map(LIST, 0xc000)
            .map(0x004c_1000, 0x1000)
            .map(NODE_BLOCKS - 4, 0x400)
            .map(RENDER_RECORDS, 2 * RENDER_RECORD_SIZE)
            .map(0x0045_dc00, 0x100)
            .map(0x004b_9000, 0x1000)
            .put_u32(MAIN_VIS + 0x14, LIST)
            .put_u32(MAIN_VIS + VIEW_KIND, 0)
            .put_u32(LIST, 4)
            .put_u16(LIST + 4, 1)
            .put_u32(LIST + 8, FLAGS)
            .put_u32(LIST + 0xc, LIST + 0x90)
            .put_u32(LIST + 0x14, LIST + 0xa0)
            .put_u16(LIST + 0xa0, u16::MAX)
            .put_u32(LIST + 0x90, ROOT_HANDLE)
            .put_u32(OBJECTS, OBJECT_POOL)
            .put_u32(OBJECT_POOL + 0x24, 12)
            .put_u32(OBJECT_POOL + 0x38, 2)
            .put_u32(OBJECT_POOL + 0x44, OBJECT_SLOTS)
            .put_u32(TAGS, TAG_TABLE)
            .put_u32(FRAME, 23)
            .put_f32(PROJECTION_DEPTH[3], 1.0)
            .put_f32(PIXEL_SCALE, 10.0)
            .put_f32(SIZE_FACTOR, 1.0)
            .put_f32(OBJECT_SIZE_SCALE, 1.0)
            .put_f32(CAMERA_DISTANCE_SCALE, 1.0)
            .put_u32(CURRENT_VIS, MAIN_VIS)
            .put_u32(sections::FIRST_PERSON_OWNER, NONE)
            .put_u32(0x004e_0344, 0)
            .put_u32(0x0050_9434, LIGHT_CACHE)
            .put_u32(0x004d_e2f4, 0x19e00)
            .put_u32(LIGHT_CACHE + 0x24, 0x100)
            .put_u32(LIGHT_CACHE + 0x38, 2)
            .put_u32(LIGHT_CACHE + 0x44, CACHE_ENTRIES)
            .put_u32(0x004e_649c, RESOURCE_CACHE)
            .put_u32(0x004e_6494, RESOURCE_CACHE)
            .put_u32(RESOURCE_CACHE + 0x38, 73)
            .put_u32(RESOURCE_CACHE + 0x64, RESOURCE_POOL)
            .put_u32(RESOURCE_POOL + 0x24, 0x18)
            .put_u32(RESOURCE_POOL + 0x38, 2)
            .put_u32(RESOURCE_POOL + 0x44, RESOURCE_SLOTS)
            .put_u32(STACK + 16, NONE)
            .put_u32(STACK + 20, 1);

        let def = 0x4141_0001;
        let hlmt = 0x4141_0002;
        let model = 0x4141_0003;
        for (datum, addr) in [(def, DEFINITION), (hlmt, HLMT), (model, MODEL)] {
            let entry = TAG_TABLE + (datum & 0xffff) * 16;
            mem.put_u32(entry + 4, datum).put_u32(entry + 8, addr);
        }
        mem.put_u32(DEFINITION + 0x38, hlmt)
            .put_u32(HLMT + 4, model)
            .put_u32(HLMT + 0xf4, NONE)
            .put_u32(MODEL + 4, 4)
            .put_u32(MODEL + 0x1c, 1)
            .put_u32(MODEL + 0x20, 0x17500)
            .put_u32(0x17508, 2)
            .put_u32(0x1750c, 0x17510)
            .put_u16(0x17510 + 14, 0)
            .put_u16(0x17520 + 14, 1)
            .put_u32(MODEL + 0x24, 2)
            .put_u32(MODEL + 0x28, SECTIONS)
            .put_u32(MODEL + 0x34, 1)
            .put_u32(MODEL + 0x38, 0x17600)
            .put_u32(MODEL + 0x48, 1)
            .put_u32(MODEL + 0x4c, 0x17700);

        let identity = [
            1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        ];
        let matrix: Vec<_> = identity.into_iter().flat_map(f32::to_le_bytes).collect();
        mem.put(0x17728, &matrix);
        for (i, (handle, body, first, parent, permutation)) in [
            (ROOT_HANDLE, ROOT, CHILD_HANDLE, NONE, 1u8),
            (CHILD_HANDLE, CHILD, NONE, ROOT_HANDLE, 0u8),
        ]
        .into_iter()
        .enumerate()
        {
            let index = i as u32;
            let slot = OBJECT_SLOTS + 12 * index;
            let entry = CACHE_ENTRIES + 0x100 * index;
            let section = SECTIONS + 0x5c * index;
            mem.put_u16(slot, (handle >> 16) as u16)
                .put_u8(slot + 3, 2)
                .put_u32(slot + 8, body)
                .put_u32(body, def)
                .put_u32(body + 4, (1 << 19) | (1 << 24))
                .put_u32(body + 0xc, NONE)
                .put_u32(body + 0x10, first)
                .put_u32(body + 0x14, parent)
                .put_f32(body + 0x3c, 1.0)
                .put_u8(body + 0xaa, 2)
                .put_u8(body + 0xb1, u8::MAX)
                .put_u32(body + 0xcc, 0x7a00_0000 | index)
                .put_u16(body + 0x112, u16::MAX)
                .put_u16(body + 0x114, 0x34)
                .put_u16(body + 0x116, 0x200)
                .put_u16(body + 0x118, 10)
                .put_u16(body + 0x11a, 0x280)
                .put_u8(body + 0x280, permutation)
                .put(body + 0x200, &matrix)
                .put_u32(entry + 4, handle)
                .put_u32(entry + 0x44, handle)
                .put_u32(entry + 0x4c, NONE)
                .put_u8(entry + 0xbc, u8::MAX)
                .put_u32(section + 0x34, 0x18500)
                .put_u8(section + 0x38 + 0x1e, 1)
                .put_u32(section + 0x38 + 0x20, index);
        }
        let mut cpu = CpuAbi::xbox_thread();
        cpu.esi = MAIN_VIS;
        cpu.esp = STACK;
        (mem, cpu)
    }

    fn publish(mem: &mut FakeGuest, out: Output) {
        for write in out.writes {
            mem.write(write.addr, write.bytes.slice()).unwrap();
        }
    }

    #[test]
    fn setup_emits_parent_then_child_and_publishes_palette_and_cache() {
        let (mut mem, cpu) = fixture();
        let out = setup(&mem, &cpu, 1.0).unwrap();
        assert_eq!(out.al, Some(0));
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 0);
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
        assert_eq!(mem.u32(MAIN_COUNT).unwrap(), 2);
        let record = |index: u32| RENDER_RECORDS + index * RENDER_RECORD_SIZE;
        let section = |index| (mem.u32(record(index) + 0xc).unwrap() >> 9) & 0x1ff;
        assert_eq!([section(0), section(1)], [1, 0]);
        let bytes = BLOCK_MATRICES + MATRIX_STRIDE;
        assert_eq!(mem.u32(NODE_BLOCKS - 4).unwrap(), 2 * bytes);
        assert_eq!(mem.u32(record(0) + 8).unwrap(), 0);
        assert_eq!(mem.u32(record(1) + 8).unwrap(), bytes);
        assert_eq!(mem.u16(NODE_BLOCKS).unwrap(), 1);
        assert_eq!(mem.f32(NODE_BLOCKS + BLOCK_MATRICES).unwrap(), 1.0);
        for i in 0..2 {
            let entry = CACHE_ENTRIES + i * 0x100;
            assert_eq!(mem.u32(entry + 0xc).unwrap(), 23);
            assert_eq!(mem.u8(entry + 0xbc).unwrap(), 5);
            assert_eq!(mem.u8(entry + 0xc0).unwrap(), (1 - i) as u8);
            assert_eq!(mem.u32(RESOURCE_SLOTS + i * 0x18 + 0x14).unwrap(), 73);
        }
    }

    #[test]
    fn object_and_child_choose_detail_by_distance_at_output_resolution() {
        let run = |scale, pose_mask| {
            let (mut mem, cpu) = fixture();
            mem.put_u32(ROOT + 4, 1 << 24)
                .put_u32(CHILD + 4, 1 << 24)
                .put_u32(HLMT + 0xc4, pose_mask)
                .put_u8(FORCE_FULL_LOD, 1)
                .put_f32(0x0044_ae90, 1.0) // override's local LOD multiplier
                .put_f32(0x0044_ae88, 0.1) // hysteresis
                .put_f32(0x0045_dc78, 1.0)
                .put_u32(0x004b_a01c, 5)
                .put_f32(records::PIXEL_QUANTIZE, 0.5)
                .put_f32(CAMERA, -100.0);
            for (i, threshold) in [55.0, 35.0, 20.0, 15.0, 5.0, 0.0].into_iter().enumerate() {
                mem.put_f32(HLMT + 0x34 + 4 * i as u32, threshold);
            }
            let output = setup(&mem, &cpu, scale).unwrap();
            publish(&mut mem, output);
            assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
            let lods = [0, 1].map(|i| mem.u8(CACHE_ENTRIES + 0x100 * i + 0xbc).unwrap());
            let bytes = [0, 1].map(|i| mem.u8(0x004c_0b84 + i).unwrap());
            (lods, bytes)
        };
        assert_eq!(run(1.0, 0), ([0; 2], [5; 2]));
        assert_eq!(run(4.0, 0), ([2; 2], [20; 2]));
        assert_eq!(run(4.0 * 1.5, 0), ([3; 2], [30; 2]));
        assert_eq!(run(4.0, 1), ([0; 2], [20; 2]));
    }

    #[test]
    fn setup_retries_the_prior_lod_when_the_requested_section_is_unavailable() {
        let (mut mem, cpu) = fixture();
        let resource = SECTIONS + 0x5c + 0x38;
        mem.put_u8(resource + 0x1e, 0)
            .put_u32(resource + 8, 0)
            .put_u8(CACHE_ENTRIES + 0xbc, 4)
            .put_u8(CACHE_ENTRIES + 0xc0, 1);
        let out = setup(&mem, &cpu, 1.0).unwrap();
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
        assert_eq!((mem.u32(RENDER_RECORDS + 0xc).unwrap() >> 9) & 0x1ff, 0);
        assert_eq!(mem.u8(CACHE_ENTRIES + 0xbc).unwrap(), 4);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0x4c).unwrap(), NONE);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0xc).unwrap(), 23);
    }

    #[test]
    fn incomplete_child_discards_the_parent_plan() {
        let (mut mem, cpu) = fixture();
        let child_resource = SECTIONS + 0x38;
        mem.put_u8(child_resource + 0x1e, 0)
            .put_u32(child_resource + 8, 16)
            .put_u32(child_resource + 0x20, NONE);
        assert!(matches!(setup(&mem, &cpu, 1.0), Err(Refusal::Residency)));
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 0);
        assert_eq!(mem.u32(NODE_BLOCKS - 4).unwrap(), 0);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0xc).unwrap(), 0);
    }

    #[test]
    fn first_person_emits_its_slot_and_leaves_the_caller_a_retry() {
        let (mut mem, mut cpu) = fixture();
        let first = sections::FIRST_PERSON_MODELS;
        let pose = mem.bytes(ROOT + 0x200, 0x34).unwrap();
        mem.put_u32(sections::FIRST_PERSON_OWNER, ROOT_HANDLE)
            .put_u16(sections::FIRST_PERSON_COUNT, 1)
            .put_u32(first, 0x4141_0003)
            .put_u32(first + 4, ROOT_HANDLE)
            .put_u32(first + 8, 1)
            .put(first + 0xc, &pose)
            .put_u32(0x004b_9ed8, NONE)
            .put_u32(LIGHT_VIS + 0x14, LIST)
            .put_u32(LIGHT_VIS + VIEW_KIND, 1)
            .put_u32(CURRENT_VIS, LIGHT_VIS);
        cpu.esi = LIGHT_VIS;
        let out = setup(&mem, &cpu, 1.0).unwrap();
        assert_eq!(out.al, Some(1));
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 1);
        assert_eq!(mem.u32(MAIN_COUNT).unwrap(), 0);
        assert_eq!(mem.u32(RENDER_RECORDS).unwrap() & 0xc00, 0xc00);
        assert_eq!((mem.u32(RENDER_RECORDS + 0xc).unwrap() >> 9) & 0x1ff, 0);

        mem.put_u32(STACK + 8, 1);
        let retry = setup(&mem, &cpu, 1.0).unwrap();
        assert_eq!(retry.al, Some(0));
        publish(&mut mem, retry);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 3);
        assert_eq!(
            mem.u32(RENDER_RECORDS + RENDER_RECORD_SIZE).unwrap() & 0xc06,
            4
        );
    }

    #[test]
    fn zero_opacity_marks_only_the_low_visibility_byte_and_returns_retry() {
        let (mut mem, mut cpu) = fixture();
        mem.put_u32(ROOT + 4, 0)
            .put_u32(DEFINITION + 0x38, NONE)
            .put_u32(sections::FIRST_PERSON_OWNER, ROOT_HANDLE)
            .put_u16(FLAGS, 0x84a0)
            .put_u32(LIGHT_VIS + 0x14, LIST)
            .put_u32(LIGHT_VIS + VIEW_KIND, 1);
        cpu.esi = LIGHT_VIS;
        let out = setup(&mem, &cpu, 1.0).unwrap();
        assert_eq!(out.al, Some(1));
        assert_eq!((out.ecx, out.edx), (Some(FLAGS), Some(FLAGS)));
        assert_eq!(mem.u16(FLAGS).unwrap(), 0x84a0);
        publish(&mut mem, out);
        assert_eq!(mem.u16(FLAGS).unwrap(), 0x84a1);

        mem.put_u32(STACK + 8, 1);
        assert_eq!(setup(&mem, &cpu, 1.0).unwrap().al, Some(0));
    }
}
