//! Object sections, per-view permutation history, and palette reservation.

use super::staged::Staged;
use super::*;
use super::{lighting, palette, residency};

pub(super) const FIRST_PERSON_OWNER: u32 = 0x004c_1bd0;
pub(super) const FIRST_PERSON_COUNT: u32 = FIRST_PERSON_OWNER + 4;
pub(super) const FIRST_PERSON_MODELS: u32 = FIRST_PERSON_OWNER + 8;
const FIRST_PERSON_STRIDE: u32 = 0xd0c;
const MODEL_LIMIT: u32 = 5;
const POSE_OVERRIDE_LOCK: u32 = 0x0047_989c;
const SKY_OVERRIDE: u32 = 0x004b_9ee9;
const SKY_CLUSTER: u32 = 0x004b_9eec;
const VIEW_PLAYER: u32 = 0x004b_9ed8;
const PLAYER_WINDOWS: u32 = 0x004e_d284;
const MAX_LOD: u32 = 0x004b_a01c;
const VIEW_INDEX: u32 = 0x004b_a050;
const LOD_FRACTION_MAX: u32 = 0x0045_dc78;
const LOD_KEEP_COARSER: u32 = 0x0044_ae88;
const LOD_KEEP_FINER: u32 = 0x0045_dc5c;
const LOCAL_LOD_SCALE: u32 = 0x0044_ae90;
const OBJECT_LOD_SCALE: u32 = 0x0045_e384;
const ABSENT: u8 = u8::MAX;
const REGION_LIMIT: usize = 16;

#[derive(Default)]
pub(super) struct Prepared {
    pub first_person: bool,
    pub effect_flags: u32,
    pub overlay: Option<Overlay>,
    pub variant_overlays: [Option<Overlay>; 2],
    pub function_overlay: Option<Overlay>,
    pub models: Vec<Model>,
}

pub(super) struct Overlay {
    pub attachment: u32,
    pub alpha: f32,
}

pub(super) struct Model {
    pub datum: u32,
    pub lighting: u32,
    pub palette: u32,
    pub selector: u16,
    pub node_count: u8,
    pub draw_flags: u32,
    pub sections: Vec<u8>,
    pub attributes: [u16; REGION_LIMIT],
}

pub(super) fn prepare(
    m: &mut Staged<'_>,
    scene: Scene,
    handle: u32,
    state: ObjectState,
) -> Result<Prepared> {
    if suppressed(m, scene.objects, handle)? {
        return Ok(Prepared::default());
    }
    let slot = scene.objects.slot(m, handle)?;
    let body = scene.objects.body(m, handle)?;
    let first_person = !scene.retry && handle == m.u32(FIRST_PERSON_OWNER)?;
    let mut prepared = Prepared {
        first_person,
        ..Prepared::default()
    };
    let definition = tag(m, scene.tags, m.u32(body)?)?;
    let hlmt = m.u32(at(definition, 0x38)?)?;
    if hlmt == NONE {
        return Ok(prepared);
    }
    let hlmt = tag(m, scene.tags, hlmt)?;
    let datum = m.u32(at(hlmt, 4)?)?;
    let slots = if first_person {
        m.u16(FIRST_PERSON_COUNT)? as i16
    } else if datum != NONE {
        1
    } else {
        0
    };
    if slots <= 0 {
        return Ok(prepared);
    }
    let slots = slots as u32;
    if slots > MODEL_LIMIT {
        return Err(Refusal::ObjectModel);
    }
    if scene.kind == 0 && datum != NONE {
        let model = tag(m, scene.tags, datum)?;
        if m.i32(at(model, 0x74)?)? > 0 {
            let extra = ptr(m, at(model, 0x78)?)?;
            if residency::touch(m, at(extra, 0x34)?, true)? {
                return Err(Refusal::ObjectModel);
            }
        }
    }
    effects(m, scene, hlmt, body, &mut prepared)?;
    if first_person {
        let player = m.u32(VIEW_PLAYER)?;
        if player != NONE {
            let offset = player.checked_mul(0x94).ok_or(Refusal::Layout)?;
            if m.u16(at(at(ptr(m, PLAYER_WINDOWS)?, offset)?, 0x42)?)? != u16::MAX {
                return Ok(prepared);
            }
        }
    }
    let selector = if m.u8(at(slot, 3)?)? == 6 {
        m.u16(at(body, 0x134)?)?
    } else {
        u16::MAX
    };
    // Hysteresis and the selected LOD are stored on the requested object.
    // A first-person slot can have a separate palette and lighting owner.
    let view = m.i32(VIEW_INDEX)?;
    let root_model = if first_person {
        0
    } else {
        tag(m, scene.tags, datum)?
    };
    let initial = if first_person || m.u32(at(body, 4)?)? & (1 << 19) != 0 {
        5
    } else {
        let old = if (0..4).contains(&view) {
            let entry = lighting::cache(m, handle, body, scene.frame)?;
            m.u8(at(entry, 0xbc + view as u32)?)?
        } else {
            ABSENT
        };
        choose_lod(m, body, hlmt, state.distance, old, scene.lod_scale)?
    };
    let mut current = initial;
    for i in 0..slots {
        let (model_datum, owner, pose, slot_flags) = if first_person {
            let row = at(FIRST_PERSON_MODELS, i * FIRST_PERSON_STRIDE)?;
            (
                m.u32(row)?,
                m.u32(at(row, 4)?)?,
                Some(at(row, 0xc)?),
                u32::from(m.u8(at(row, 8)?)? & 1) << 11,
            )
        } else {
            (datum, handle, None, 0)
        };
        let model = if first_person {
            tag(m, scene.tags, model_datum)?
        } else {
            root_model
        };
        let owner_body = scene.objects.body(m, owner)?;
        let definition = tag(m, scene.tags, m.u32(owner_body)?)?;
        if let Some(mut selected) = prepare_model(
            m,
            scene,
            state,
            Slot {
                requested: handle,
                requested_body: body,
                hlmt,
                model,
                model_datum,
                owner,
                owner_body,
                definition,
                pose,
                initial,
                view,
                single_slot: slots == 1,
                index: i,
                selector,
            },
            &mut current,
        )? {
            selected.draw_flags = (prepared.effect_flags & 4) << 13
                | (prepared.effect_flags & 0x40) << 7
                | slot_flags;
            prepared.models.push(selected);
        }
    }
    Ok(prepared)
}

struct Slot {
    requested: u32,
    requested_body: u32,
    hlmt: u32,
    model: u32,
    model_datum: u32,
    owner: u32,
    owner_body: u32,
    definition: u32,
    pose: Option<u32>,
    initial: u8,
    view: i32,
    single_slot: bool,
    index: u32,
    selector: u16,
}

fn prepare_model(
    m: &mut Staged<'_>,
    scene: Scene,
    state: ObjectState,
    slot: Slot,
    current: &mut u8,
) -> Result<Option<Model>> {
    let Slot {
        requested,
        requested_body,
        hlmt,
        model,
        model_datum,
        owner,
        owner_body,
        definition,
        pose,
        initial,
        view,
        single_slot,
        index,
        selector,
    } = slot;
    let entry = lighting::cache(m, owner, owner_body, scene.frame)?;
    let old = if (0..4).contains(&view) {
        m.u8(at(entry, 0xbc + view as u32)?)?
    } else {
        ABSENT
    };
    if old != ABSENT && old > 5 {
        return Err(Refusal::ObjectModel);
    }
    let first_person = pose.is_some();
    let history = if first_person || !(0..4).contains(&view) {
        None
    } else {
        Some(at(entry, 0xc0 + 0x10 * view as u32)?)
    };
    let token = scene.frame & 0x3fff_ffff | (view as u32).wrapping_shl(30);
    let saved_palette = m.u32(at(entry, 0x4c)?)?;
    let reusable = old != ABSENT && saved_palette != NONE && m.u32(at(entry, 0x50)?)? == token;
    let write_lod = |m: &mut Staged<'_>, lod| -> Result<()> {
        if (0..4).contains(&view) {
            let cache = lighting::cache(m, requested, requested_body, scene.frame)?;
            m.set_u8(at(cache, 0xbc + view as u32)?, lod)?;
        }
        Ok(())
    };
    let desired = if reusable { old } else { *current };
    *current = desired;
    let mut selected = pick(
        m,
        model,
        requested_body,
        desired,
        history,
        true,
        first_person,
    )?;
    let mut chosen = desired;
    if selected.complete {
        write_lod(m, chosen)?;
    } else if old != ABSENT {
        selected = pick(m, model, requested_body, old, history, false, first_person)?;
        if selected.complete {
            chosen = old;
            *current = chosen;
        }
    }
    if !selected.complete {
        write_lod(m, ABSENT)?;
        return Ok(None);
    }
    if index != 0 && chosen != initial {
        return Ok(None);
    }
    let attributes = if single_slot {
        attributes(m, hlmt, requested_body, selected.sections.len())?
    } else {
        [0; REGION_LIMIT]
    };
    let count = palette::size(m, model, chosen, &selected.sections)?;
    let palette = if !first_person && reusable && !selected.varied && chosen == initial {
        saved_palette
    } else {
        let Some(offset) = palette::reserve(m, count)? else {
            return Ok(None);
        };
        if !first_person && !selected.varied && chosen == initial {
            m.set_u32(at(entry, 0x50)?, token)?;
            m.set_u32(at(entry, 0x4c)?, offset)?;
        }
        if state.lock_pose
            && (m.u32(POSE_OVERRIDE_LOCK)? != 0
                || m.u8(SKY_OVERRIDE)? != 0 && m.u32(SKY_CLUSTER)? != NONE)
        {
            return Err(Refusal::Palette);
        }
        let pose = if let Some(pose) = pose {
            if m.u32(at(model, 0x48)?)? > 64 {
                return Err(Refusal::Palette);
            }
            pose
        } else {
            let object_slot = scene.objects.slot(m, owner)?;
            palette::object_pose(m, model, owner_body, object_slot, hlmt, &selected.sections)?
        };
        palette::fill(m, model, chosen, &selected.sections, pose, offset, count)?;
        lighting::refresh(
            m,
            owner,
            owner_body,
            definition,
            entry,
            scene.frame,
            state.pixels,
        )?;
        offset
    };
    if m.u32(at(entry, 4)?)? != owner {
        return Err(Refusal::Lighting);
    }
    m.set_u32(at(entry, 0xc)?, scene.frame)?;
    Ok(Some(Model {
        datum: model_datum,
        lighting: at(entry, 0x44)?,
        palette,
        selector,
        node_count: count as u8,
        draw_flags: 0,
        sections: selected.sections,
        attributes,
    }))
}

fn suppressed(m: &dyn GuestMemory, objects: Pool, mut handle: u32) -> Result<bool> {
    for _ in 0..objects.end {
        let slot = objects.slot(m, handle)?;
        let body = ptr(m, at(slot, 8)?)?;
        if m.u8(at(slot, 2)?)? & 0x10 != 0 || m.u8(at(body, 4)?)? & 1 != 0 {
            return Ok(true);
        }
        handle = m.u32(at(body, 0x14)?)?;
        if handle == NONE {
            return Ok(false);
        }
    }
    Err(Refusal::Layout)
}

fn effects(
    m: &dyn GuestMemory,
    scene: Scene,
    hlmt: u32,
    body: u32,
    prepared: &mut Prepared,
) -> Result<()> {
    let flags = m.u32(at(hlmt, 0x9c)?)?;
    let kind = m.u8(at(body, 0xaa)?)?;
    let alpha = if flags & 1 != 0 {
        prepared.effect_flags = 6;
        Some(m.f32(0x0045_e0dc)?)
    } else if kind <= 1 {
        let alpha = m.f32(at(body, 0x2b0)?)?;
        if alpha > m.f32(DISTANCE_EPSILON)? {
            prepared.effect_flags = 2;
            Some(alpha)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(alpha) = alpha {
        prepared.effect_flags |= if prepared.first_person {
            8
        } else {
            (flags & 4) << 1 | (flags & 2) << 3
        };
        prepared.overlay = Some(Overlay {
            attachment: m.u32(at(ptr(m, 0x0048_5a80)?, 0xd0)?)?,
            alpha,
        });
    }
    if scene.kind == 0 {
        if m.i32(at(hlmt, 0x60)?)? > 0 {
            let row = ptr(m, at(hlmt, 0x64)?)?;
            let (primary, secondary) = if prepared.first_person {
                (0x80, 0xec)
            } else {
                (0x88, 0xf4)
            };
            let strength = m.f32(at(body, 0xf0)?)?;
            let one = m.f32(ONE)?;
            let variants = [
                (m.f32(at(body, 0xf4)?)? > 0.0, primary, strength),
                (strength > one, secondary, one - strength),
            ];
            for (out, (active, field, alpha)) in prepared.variant_overlays.iter_mut().zip(variants)
            {
                if active {
                    let attachment = m.u32(at(row, field)?)?;
                    if attachment != NONE {
                        *out = Some(Overlay { attachment, alpha });
                    }
                }
            }
        }
        let attachment = m.u32(at(hlmt, 0xf4)?)?;
        if attachment != NONE {
            // Zero selects the constant object function. Other functions
            // can invoke animation and object-function callbacks.
            if m.u32(at(hlmt, 0xf8)?)? != 0 || m.u16(at(body, 0xc0)?)? & 0x200 != 0 {
                return Err(Refusal::ObjectModel);
            }
            let alpha = m.f32(ONE)?;
            if alpha > m.f32(DISTANCE_EPSILON)? {
                prepared.function_overlay = Some(Overlay { attachment, alpha });
            }
        }
    }
    Ok(())
}

/// `0x0004bcc0` and `0x0004c450`. The cutoffs at `hlmt + 0x34` are camera
/// distances, farthest first.
fn choose_lod(
    m: &dyn GuestMemory,
    body: u32,
    hlmt: u32,
    mut distance: f32,
    old: u8,
    scale: f32,
) -> Result<u8> {
    if m.u32(at(body, 4)?)? & (1 << 19) != 0 {
        return Ok(5);
    }
    if m.u8(FORCE_FULL_LOD)? != 0 {
        distance *= m.f32(LOCAL_LOD_SCALE)?;
    }
    if m.u8(at(body, 0x10a)?)? & 4 != 0 {
        distance *= m.f32(OBJECT_LOD_SCALE)?;
    }
    distance = object_lod::model_distance(m, hlmt, distance, scale);
    let block = at(hlmt, 0x28)?;
    let mut thresholds = [0.0; 6];
    for (i, threshold) in thresholds.iter_mut().enumerate() {
        *threshold = m.f32(at(block, 0xc + 4 * i as u32)?)?;
    }
    let mut upper = m.f32(block)?;
    if !(thresholds[0] > 0.0
        && thresholds[5] == 0.0
        && thresholds[..5].windows(2).all(|pair| pair[0] >= pair[1]))
    {
        for (threshold, addr) in thresholds.iter_mut().zip([
            0x0045_dc00,
            0x0045_dc18,
            0x0045_dbfc,
            0x0045_dc10,
            0x0045_dc0c,
        ]) {
            *threshold = m.f32(addr)?;
        }
        thresholds[5] = 0.0;
        upper = 0.0;
    }
    let max_fraction = m.f32(LOD_FRACTION_MAX)?;
    let limit = m.i32(MAX_LOD)?;
    if !(0..=5).contains(&limit)
        || max_fraction < 0.0
        || ![upper, distance, max_fraction]
            .into_iter()
            .chain(thresholds)
            .all(f32::is_finite)
    {
        return Err(Refusal::ObjectModel);
    }
    let (lod, fraction) = lod_fraction(distance, thresholds, upper, max_fraction);
    if !fraction.is_finite() {
        return Err(Refusal::ObjectModel);
    }
    let lod = hysteresis(
        lod,
        fraction,
        old,
        m.f32(LOD_KEEP_COARSER)?,
        m.f32(LOD_KEEP_FINER)?,
    );
    Ok(lod.min(limit as u8))
}

fn lod_fraction(distance: f32, thresholds: [f32; 6], upper: f32, cap: f32) -> (u8, f32) {
    let lod = thresholds
        .iter()
        .position(|&cut| distance > cut || cut == 0.0)
        .unwrap_or(5);
    if lod == 0 && upper <= thresholds[0] {
        return (0, cap);
    }
    let previous = if lod == 0 { upper } else { thresholds[lod - 1] };
    let fraction = ((previous - distance) / (previous - thresholds[lod])).clamp(0.0, cap);
    (lod as u8, fraction)
}

fn hysteresis(lod: u8, fraction: f32, old: u8, coarser: f32, finer: f32) -> u8 {
    if old == lod + 1 && fraction < coarser || old.checked_add(1) == Some(lod) && fraction > finer {
        old
    } else {
        lod
    }
}

struct Selection {
    sections: Vec<u8>,
    complete: bool,
    varied: bool,
}

fn pick(
    m: &mut Staged<'_>,
    model: u32,
    body: u32,
    lod: u8,
    history: Option<u32>,
    request: bool,
    first_person: bool,
) -> Result<Selection> {
    let count = m.u32(at(model, 0x1c)?)? as usize;
    if count > REGION_LIMIT {
        return Err(Refusal::ObjectModel);
    }
    let mut selection = Selection {
        sections: vec![ABSENT; count],
        complete: true,
        varied: false,
    };
    if count == 0 {
        return Ok(selection);
    }
    let regions = ptr(m, at(model, 0x20)?)?;
    let current = if first_person {
        vec![0; count]
    } else {
        let size = m.u16(at(body, 0x118)?)? as i16;
        let start = m.u16(at(body, 0x11a)?)? as i16;
        if start < 0 || size < 0 || count > size as usize / 10 {
            return Err(Refusal::ObjectModel);
        }
        m.bytes(at(body, start as u32)?, count)?
    };
    // Touch every requested region before inspecting the previous permutations.
    for (i, &permutation) in current.iter().enumerate() {
        let region = at(regions, i as u32 * 16)?;
        if permutation == ABSENT || i32::from(permutation) >= m.i32(at(region, 8)?)? {
            continue;
        }
        let section = section_index(m, region, permutation, lod)?;
        let section_addr = palette::section_addr(m, model, u32::from(section))?;
        if residency::touch(m, at(section_addr, 0x38)?, request)? {
            selection.sections[i] = section;
        }
    }
    for (i, (&permutation, section)) in current.iter().zip(&mut selection.sections).enumerate() {
        if permutation == ABSENT {
            continue;
        }
        let historical = if first_person {
            None
        } else {
            Some(at(history.ok_or(Refusal::Lighting)?, i as u32)?)
        };
        if *section != ABSENT {
            if let Some(historical) = historical {
                m.set_u8(historical, permutation)?;
            }
            continue;
        }
        let previous = if let Some(historical) = historical {
            m.u8(historical)?
        } else {
            0
        };
        if previous == ABSENT {
            selection.varied = true;
            continue;
        }
        let region = at(regions, i as u32 * 16)?;
        let prior_section = section_index(m, region, previous, lod)?;
        let section_addr = palette::section_addr(m, model, u32::from(prior_section))?;
        if residency::touch(m, at(section_addr, 0x38)?, false)? {
            *section = prior_section;
            selection.varied = true;
        } else {
            selection.complete = false;
        }
    }
    Ok(selection)
}

fn section_index(m: &dyn GuestMemory, region: u32, permutation: u8, lod: u8) -> Result<u8> {
    if permutation > i8::MAX as u8 || i32::from(permutation) >= m.i32(at(region, 8)?)? {
        return Err(Refusal::ObjectModel);
    }
    let row = at(ptr(m, at(region, 0xc)?)?, u32::from(permutation) * 16)?;
    let section = m.u16(at(row, 4 + 2 * u32::from(lod))?)?;
    if section >= u16::from(ABSENT) {
        return Err(Refusal::ObjectModel);
    }
    Ok(section as u8)
}

fn attributes(
    m: &dyn GuestMemory,
    hlmt: u32,
    body: u32,
    count: usize,
) -> Result<[u16; REGION_LIMIT]> {
    let mut attrs = [0; REGION_LIMIT];
    let index = m.u8(at(body, 0xb1)?)?;
    if index == ABSENT || count == 0 {
        return Ok(attrs);
    }
    if index > i8::MAX as u8
        || u32::from(index) >= m.u32(at(hlmt, 0x50)?)?
        || count as u32 > m.u32(at(hlmt, 0x70)?)?
    {
        return Err(Refusal::ObjectModel);
    }
    let variant = at(ptr(m, at(hlmt, 0x54)?)?, u32::from(index) * 0x38)?;
    for (i, attr) in attrs[..count].iter_mut().enumerate() {
        let region = m.u8(at(variant, i as u32 + 4)?)?;
        if region == ABSENT {
            continue;
        }
        if region > i8::MAX as u8 || u32::from(region) >= m.u32(at(variant, 0x14)?)? {
            return Err(Refusal::ObjectModel);
        }
        let row = at(ptr(m, at(variant, 0x18)?)?, u32::from(region) * 20)?;
        *attr = m.u16(at(row, 0x10)?)?;
    }
    Ok(attrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_order_uses_strict_thresholds_and_asymmetric_hysteresis() {
        let thresholds = [100.0, 80.0, 60.0, 40.0, 20.0, 0.0];
        assert_eq!(lod_fraction(101.0, thresholds, 0.0, 1.0), (0, 1.0));
        assert_eq!(lod_fraction(100.0, thresholds, 0.0, 1.0), (1, 0.0));
        assert_eq!(lod_fraction(70.0, thresholds, 0.0, 1.0), (2, 0.5));
        assert_eq!(hysteresis(2, 0.25, 3, 0.5, 0.75), 3);
        assert_eq!(hysteresis(2, 0.5, 3, 0.5, 0.75), 2);
        assert_eq!(hysteresis(2, 0.75, 1, 0.5, 0.75), 2);
        assert_eq!(hysteresis(2, 0.9, 1, 0.5, 0.75), 1);
        assert_eq!(hysteresis(2, 0.9, ABSENT, 0.5, 0.75), 2);
    }
}
