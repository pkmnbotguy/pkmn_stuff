//! Champions stat formula (Showdown's `champions` mod, `statModify`): stat points instead of EVs,
//! perfect IVs, level 50.
//!   HP    = base + points + 75
//!   other = (base + points + 20), then x1.1 / x0.9 for the nature (truncated)

use crate::dex::{Nature, HP};

pub fn champions_stats(base: &[u16; 6], points: &[u16; 6], nature: Nature) -> [u16; 6] {
    let mut out = [0u16; 6];
    for i in 0..6 {
        if i == HP {
            out[i] = base[i] + points[i] + 75;
            continue;
        }
        let mut stat = (base[i] + points[i] + 20) as u32;
        if nature.plus == Some(i) {
            stat = (stat * 110 & 0xFFFF) / 100;
        } else if nature.minus == Some(i) {
            stat = (stat * 90 & 0xFFFF) / 100;
        }
        out[i] = stat as u16;
    }
    out
}

/// A stat with its boost stage applied (Showdown's `calculateStat` boost table).
pub fn boosted(stat: u32, stage: i8) -> u32 {
    let s = stage.clamp(-6, 6) as i32;
    if s >= 0 {
        stat * (2 + s as u32) / 2
    } else {
        stat * 2 / (2 + (-s) as u32)
    }
}
