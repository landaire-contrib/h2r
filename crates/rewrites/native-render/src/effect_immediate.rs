//! Researched effect leaves. Ownership ends at one BEGIN/END pair;
//! parent handlers, visibility queries and simulation remain guest-owned.
use crate::particles::Vertex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Billboard,
    DepthBillboard,
    WeatherLayer,
    WeatherParticles,
    MaterialStrip,
}
impl Site {
    pub const ALL: [Self; 5] = [
        Self::Billboard,
        Self::DepthBillboard,
        Self::WeatherLayer,
        Self::WeatherParticles,
        Self::MaterialStrip,
    ];
    pub const fn caller_offset(self) -> Option<u32> {
        match self {
            Self::Billboard => Some(0x24),
            Self::DepthBillboard => Some(0x30),
            Self::MaterialStrip => Some(0x40),
            _ => None,
        }
    }
    pub fn accepts_caller(self, caller: Option<u32>) -> bool {
        match self {
            Self::Billboard => matches!(caller, Some(0x0002_f24c | 0x0004_fddd)),
            Self::DepthBillboard => caller == Some(0x0004_fd9d),
            Self::MaterialStrip => matches!(caller, Some(0x0024_45fa | 0x002b_e647)),
            _ => caller.is_none(),
        }
    }
}

/// These reasons are diagnostic names, not permission to skip any handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    UnownedCaller,
    ImmediateDeclaration,
    ImmediateGeometry,
    WeatherPointRasterization,
}
impl Refused {
    pub const fn name(self) -> &'static str {
        match self {
            Self::UnownedCaller => "UnownedCaller",
            Self::ImmediateDeclaration => "ImmediateDeclaration",
            Self::ImmediateGeometry => "ImmediateGeometry",
            Self::WeatherPointRasterization => "WeatherPointRasterization",
        }
    }
}

/// Decode the resolved immediate register stream, preserving written bits
/// and filling *unwritten* registers from issue-time defaults. Declaration
/// verification belongs to the adapter. No independent guest math replay.
pub fn decode(
    site: Site,
    primitive: u32,
    count: u32,
    bytes: &[u8],
    written: u16,
    defaults: [[f32; 4]; 16],
) -> Result<(Vec<Vertex>, Vec<u32>), Refused> {
    if site == Site::WeatherParticles && primitive == 1 {
        return Err(Refused::WeatherPointRasterization);
    }
    let valid = match site {
        Site::Billboard | Site::DepthBillboard => primitive == 7 && count == 4,
        Site::WeatherLayer => primitive == 9 && count == 4,
        Site::WeatherParticles => primitive == 2 && count > 0 && count.is_multiple_of(2),
        Site::MaterialStrip => primitive == 6 && count >= 4 && count.is_multiple_of(2),
    };
    if !valid || count > 65536 || bytes.len() != count as usize * 256 || written & 1 == 0 {
        return Err(Refused::ImmediateGeometry);
    }
    let vertices = bytes
        .chunks_exact(256)
        .map(|chunk| {
            let mut registers = defaults;
            for (r, register) in registers.iter_mut().enumerate() {
                if written & (1 << r) != 0 {
                    for (c, value) in register.iter_mut().enumerate() {
                        let offset = r * 16 + c * 4;
                        *value = f32::from_bits(u32::from_le_bytes(
                            chunk[offset..offset + 4].try_into().unwrap(),
                        ));
                    }
                }
            }
            Vertex { registers }
        })
        .collect();
    let indices = match primitive {
        7 => vec![0, 1, 2, 0, 2, 3],
        9 => vec![0, 1, 3, 0, 3, 2],
        _ => (0..count).collect(),
    };
    Ok((vertices, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_and_rasterization_refuse_unknown_work() {
        assert!(Site::Billboard.accepts_caller(Some(0x2f24c)));
        assert!(!Site::Billboard.accepts_caller(Some(0x2f24d)));
        assert!(!Site::DepthBillboard.accepts_caller(Some(0x4fddd)));
        assert_eq!(
            decode(Site::WeatherParticles, 1, 1, &[0; 256], 1, [[0.; 4]; 16]),
            Err(Refused::WeatherPointRasterization)
        );
        assert_eq!(
            decode(Site::WeatherParticles, 2, 3, &[0; 768], 1, [[0.; 4]; 16]),
            Err(Refused::ImmediateGeometry)
        );
    }
    #[test]
    fn material_strip_owns_only_researched_callers_and_preserves_degenerates() {
        assert!(Site::MaterialStrip.accepts_caller(Some(0x2445fa)));
        assert!(Site::MaterialStrip.accepts_caller(Some(0x2be647)));
        assert!(!Site::MaterialStrip.accepts_caller(Some(0x244610)));
        let bytes = vec![0; 1536];
        let (_, indices) = decode(Site::MaterialStrip, 6, 6, &bytes, 1, [[0.; 4]; 16]).unwrap();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5]);
        assert!(decode(Site::MaterialStrip, 6, 5, &bytes[..1280], 1, [[0.; 4]; 16]).is_err());
        assert!(decode(Site::MaterialStrip, 7, 6, &bytes, 1, [[0.; 4]; 16]).is_err());
    }
    #[test]
    fn keeps_bits_defaults_and_quad_strip_winding() {
        let mut bytes = vec![0; 1024];
        bytes[..4].copy_from_slice(&0x7fc00042u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&0x80000000u32.to_le_bytes());
        let (v, i) = decode(Site::WeatherLayer, 9, 4, &bytes, 1, [[0.25; 4]; 16]).unwrap();
        assert_eq!(v[0].registers[0][0].to_bits(), 0x7fc00042);
        assert_eq!(v[0].registers[0][1].to_bits(), 0x80000000);
        assert_eq!(v[0].registers[3], [0.25; 4]);
        assert_eq!(i, [0, 1, 3, 0, 3, 2]);
        let (_, lines) = decode(Site::WeatherParticles, 2, 4, &bytes, 1, [[0.; 4]; 16]).unwrap();
        assert_eq!(lines, [0, 1, 2, 3]);
    }
}
