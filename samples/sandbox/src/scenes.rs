//! Game-owned fluid acceptance scenes; emitted as ordinary replayable edits.
use crate::materials::{SAND, WALL, WATER};
use charon_sand::Command;
pub fn sea(width: usize, height: usize) -> Vec<Command> {
    let bank = width / 2;
    let lip = height / 2;
    let thickness = (width / 60).max(1);
    let surface = lip + (height / 64).max(1);
    vec![
        Command::Clear,
        Command::FillRect {
            x: bank,
            y: lip,
            width: thickness,
            height: height - lip,
            material: WALL,
        },
        Command::FillRect {
            x: 1,
            y: surface,
            width: bank - 1,
            height: (height - 1).saturating_sub(surface),
            material: WATER,
        },
        Command::FillRect {
            x: bank + thickness,
            y: surface,
            width: width.saturating_sub(bank + thickness + 1),
            height: (height - 1).saturating_sub(surface),
            material: SAND,
        },
    ]
}
