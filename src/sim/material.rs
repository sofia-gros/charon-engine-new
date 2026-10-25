//! Cell kinds and per-cell color synthesis.
//!
//! Each cell is two bytes: a material ID and an 8-bit `extra` field.
//! `extra` is overloaded by material — for granulars it's a randomness seed
//! for color variation, for fire/steam/acid it's the remaining life counter.
//! Two bytes per cell keeps a 1280x720 grid at ~1.8 MB, which fits in any
//! modern L2/L3 cache and is essential for the ~5 GB/s memory throughput
//! that the inner physics sweep requires.

/// Registry-local material ID. Ten standard IDs are reserved; custom IDs are
/// returned by MaterialRegistry::register. IDs must use the owning registry.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct Cell(pub(crate) u8);

/// Material IDs are local to the registry that allocated them.
pub type MaterialId = Cell;

#[allow(non_upper_case_globals)]
impl Cell {
    pub const Air: Self = Self(0);
    pub const Sand: Self = Self(1);
    pub const Block: Self = Self(2);
    pub const Water: Self = Self(3);
    pub const Stone: Self = Self(4);
    pub const Fire: Self = Self(5);
    pub const Steam: Self = Self(6);
    pub const Oil: Self = Self(7);
    pub const Gravel: Self = Self(8);
    pub const Acid: Self = Self(9);
    pub fn id(self) -> u8 {
        self.0
    }
}

impl Cell {
    /// Standard-preset reference density (custom definitions use MaterialRegistry).
    /// These integer values are retained as preset metadata for
    /// `can_displace`. Values are rounded approximations from the
    /// Engineering Toolbox and CRC Handbook; comments cite the source
    /// material so the displacement ordering is auditable.
    ///
    /// The CA only uses this for `>` comparisons, so the exact numbers
    /// don't affect performance — but tying them to real values means
    /// "what sinks in what" matches physical intuition out of the box.
    pub fn density(self) -> u16 {
        match self {
            Cell::Air => 1,      // 1.225 kg/m³ — air at sea level, 15°C
            Cell::Steam => 1,    // 0.6 kg/m³ — water vapour at 100°C, 1 atm
            Cell::Fire => 1,     // ~0.3 kg/m³ — hot combustion gases
            Cell::Oil => 870,    // 850–920 kg/m³ — light crude / olive / motor oils
            Cell::Water => 1000, // 1000 kg/m³ — pure water at 4°C (reference)
            Cell::Acid => 1100,  // ~1100 kg/m³ — 10% HCl by mass; concentrated
            //                                 strong acids reach 1840 (H₂SO₄
            //                                 98%). We pick a "strong-but-not-
            //                                 fuming" value so acid sinks in
            //                                 water — which is the correct
            //                                 behavior for HCl, HNO₃, H₂SO₄,
            //                                 and HF (the etchant we model).
            Cell::Sand => 1442,      // 1442 kg/m³ — loose dry quartz sand (USGS)
            Cell::Gravel => 1680,    // 1680 kg/m³ — typical washed pea gravel
            Cell::Stone => 2691,     // 2691 kg/m³ — granite (CRC Handbook 95th ed.)
            Cell::Block => u16::MAX, // sentinel: immovable user-controlled solid
            _ => 1,                  // custom properties live in the owning registry
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Cell::Air => "Air",
            Cell::Sand => "Sand",
            Cell::Block => "Block",
            Cell::Water => "Water",
            Cell::Stone => "Stone",
            Cell::Fire => "Fire",
            Cell::Steam => "Steam",
            Cell::Oil => "Oil",
            Cell::Gravel => "Gravel",
            Cell::Acid => "Acid",
            _ => "Custom",
        }
    }

    /// Whether this material is emissive — used by the renderer's bloom pass.
    pub fn emissive(self) -> bool {
        matches!(self, Cell::Fire | Cell::Acid)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellData {
    pub kind: Cell,
    pub extra: u8,
}

impl CellData {
    pub const AIR: Self = Self {
        kind: Cell::Air,
        extra: 0,
    };

    pub fn sand(seed: u8) -> Self {
        Self {
            kind: Cell::Sand,
            extra: seed,
        }
    }
    pub fn new(kind: Cell, extra: u8) -> Self {
        Self { kind, extra }
    }
    pub fn gravel(seed: u8) -> Self {
        Self {
            kind: Cell::Gravel,
            extra: seed,
        }
    }
    pub fn water(seed: u8) -> Self {
        Self {
            kind: Cell::Water,
            extra: seed,
        }
    }
    pub fn oil(seed: u8) -> Self {
        Self {
            kind: Cell::Oil,
            extra: seed,
        }
    }
    pub fn stone(seed: u8) -> Self {
        Self {
            kind: Cell::Stone,
            extra: seed,
        }
    }
    pub fn fire(life: u8) -> Self {
        Self {
            kind: Cell::Fire,
            extra: life,
        }
    }
    pub fn steam(life: u8) -> Self {
        Self {
            kind: Cell::Steam,
            extra: life,
        }
    }
    pub fn acid(life: u8) -> Self {
        Self {
            kind: Cell::Acid,
            extra: life,
        }
    }
    pub fn block() -> Self {
        Self {
            kind: Cell::Block,
            extra: 0,
        }
    }

    /// Synthesise the on-screen RGBA for this cell. Colors use small Bayer-
    /// style hash mixes of `extra` to avoid a flat look — sand needs visible
    /// grain variation or it reads as a single yellow rectangle.
    pub fn to_rgba(self) -> [u8; 4] {
        let e = self.extra;
        match self.kind {
            Cell::Air => [10, 11, 18, 255],

            Cell::Sand => {
                let h1 = ((e as u16).wrapping_mul(197)) as u8;
                let h2 = ((e as u16).wrapping_mul(53)) as u8;
                let r =
                    (215i16 + (h1 as i16 - 128) / 6 + (h2 as i16 - 128) / 14).clamp(178, 242) as u8;
                let g =
                    (192i16 + (h1 as i16 - 128) / 8 + (h2 as i16 - 128) / 16).clamp(155, 220) as u8;
                let b =
                    (132i16 + (h1 as i16 - 128) / 12 + (h2 as i16 - 128) / 20).clamp(95, 168) as u8;
                [r, g, b, 255]
            }

            Cell::Gravel => {
                let h1 = ((e as u16).wrapping_mul(173)) as u8;
                let h2 = ((e as u16).wrapping_mul(89)) as u8;
                let speck = if h2 > 220 { 18i16 } else { 0 };
                let r = (118i16 + (h1 as i16 - 128) / 9 + speck).clamp(92, 148) as u8;
                let g = (108i16 + (h1 as i16 - 128) / 11 + speck).clamp(82, 138) as u8;
                let b = (98i16 + (h1 as i16 - 128) / 13 + speck).clamp(72, 128) as u8;
                [r, g, b, 255]
            }

            Cell::Water => {
                let h = ((e as u16).wrapping_mul(131)) as u8;
                let r = (25i16 + (h as i16 - 128) / 20).clamp(15, 45) as u8;
                let g = (75i16 + (h as i16 - 128) / 10).clamp(55, 105) as u8;
                let b = (195i16 + (h as i16 - 128) / 8).clamp(160, 230) as u8;
                [r, g, b, 220]
            }

            Cell::Oil => {
                let h = ((e as u16).wrapping_mul(67)) as u8;
                let irid = (h as i16 - 128).abs() / 40;
                let r = (58i16 + (h as i16 - 128) / 14 + irid).clamp(38, 78) as u8;
                let g = (38i16 + (h as i16 - 128) / 18).clamp(22, 54) as u8;
                let b = (18i16 + (h as i16 - 128) / 24 + irid * 2).clamp(8, 38) as u8;
                [r, g, b, 235]
            }

            Cell::Stone => {
                let h1 = ((e as u16).wrapping_mul(211)) as u8;
                let h2 = ((e as u16).wrapping_mul(97)) as u8;
                let fleck = if h2 > 235 {
                    22i16
                } else if h2 < 20 {
                    -12
                } else {
                    0
                };
                let base = (118i16 + (h1 as i16 - 128) / 8 + fleck).clamp(85, 148) as u8;
                [
                    base,
                    (base as i16 + 3).clamp(0, 255) as u8,
                    (base as i16 + 8).clamp(0, 255) as u8,
                    255,
                ]
            }

            Cell::Fire => {
                let f = e as f32 / 120.0;
                let f2 = f * f;
                let r = (255.0 * f.clamp(0.4, 1.0)) as u8;
                let g = (60.0 + 180.0 * f2.min(1.0)) as u8;
                let b = (15.0 + 45.0 * (f2 * f).min(1.0)) as u8;
                [r, g, b, 255]
            }

            Cell::Steam => {
                let f = e as f32 / 80.0;
                let h = ((e as u16).wrapping_mul(151)) as u8;
                let tint = (h as i16 - 128) / 30;
                let v = (210i16 + tint).clamp(195, 225) as u8;
                let a = (160.0 * f.clamp(0.0, 1.0)) as u8;
                [v, v, (v as i16 + 8).min(255) as u8, a.max(15)]
            }

            Cell::Acid => {
                let f = e as f32 / 100.0;
                let pulse = ((e as f32 * 0.5).sin() * 0.5 + 0.5) * 0.15;
                let r = (30.0 + 30.0 * (1.0 - f) + pulse * 60.0) as u8;
                let g = (180.0 * f.clamp(0.25, 1.0) + pulse * 40.0).min(255.0) as u8;
                let b = (20.0 + 15.0 * pulse) as u8;
                [r, g, b, 230]
            }

            Cell::Block => [56, 60, 78, 255],
            _ => [255, 0, 255, 255], // use registry.color() for custom materials
        }
    }
}
