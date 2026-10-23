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

/// Bounded prototype cave. Content generation stays in the game, not the sand
/// plugin. A seed selects connected rooms; the starter tunnel remains walkable.
pub fn cave(width: usize, height: usize, seed: u64) -> (Vec<u8>, [f32; 2]) {
    use crate::materials::AIR;
    assert!(width >= 160 && height >= 120);
    fn random(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9e3779b97f4a7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }
    let floor = height * 2 / 3;
    let mut cells = vec![WALL; width * height];
    // Guaranteed entry route, broad enough for the 7x12 starter actor.
    for y in floor - 32..floor {
        cells[y * width + 1..y * width + width - 1].fill(AIR);
    }
    for index in 0..(width / 48).max(2) {
        let value = random(seed.wrapping_add(index as u64));
        let cx = 40 + (value as usize % (width - 80));
        let cy = floor - 24 - ((value >> 16) as usize % (height / 5));
        let rx = 15 + ((value >> 32) as usize % 18);
        let ry = 14 + ((value >> 48) as usize % 14);
        for y in cy.saturating_sub(ry).max(1)..=(cy + ry).min(height - 2) {
            for x in cx.saturating_sub(rx).max(1)..=(cx + rx).min(width - 2) {
                let dx = x.abs_diff(cx) as u64;
                let dy = y.abs_diff(cy) as u64;
                if dx * dx * (ry * ry) as u64 + dy * dy * (rx * rx) as u64
                    <= (rx * rx * ry * ry) as u64
                {
                    cells[y * width + x] = AIR;
                }
            }
        }
        // Each room joins the starter route; rooms never depend on RNG luck
        // for connectivity. A narrow sand deposit can fall into the route.
        for y in cy..floor {
            for x in cx - 6..cx + 6 {
                cells[y * width + x] = AIR;
            }
        }
        for y in floor - 3..floor {
            for x in cx..cx + 5 {
                cells[y * width + x] = SAND;
            }
        }
    }
    let spawn = [width / 8, floor - 12];
    // Clear a fixed landing zone last, including any generated deposit.
    for y in floor - 24..floor {
        cells[y * width + spawn[0] - 4..y * width + spawn[0] + 16].fill(AIR);
    }
    (cells, [spawn[0] as f32, spawn[1] as f32])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeded_caves_keep_boundaries_safe_spawn_and_connected_air() {
        use crate::materials::AIR;
        for (width, height) in [(160, 120), (480, 270)] {
            for seed in [0, 1, 42, u64::MAX] {
                let (cells, spawn) = cave(width, height, seed);
                assert_eq!(cave(width, height, seed).0, cells);
                assert!((0..height)
                    .all(|y| cells[y * width] == WALL && cells[y * width + width - 1] == WALL));
                assert!(cells[(height - 1) * width..].iter().all(|&c| c == WALL));
                for y in spawn[1] as usize..spawn[1] as usize + 12 {
                    for x in spawn[0] as usize..spawn[0] as usize + 7 {
                        assert_eq!(cells[y * width + x], AIR);
                    }
                }
                let mut visited = vec![false; cells.len()];
                let start = spawn[1] as usize * width + spawn[0] as usize;
                let mut queue = std::collections::VecDeque::from([start]);
                visited[start] = true;
                while let Some(index) = queue.pop_front() {
                    for next in [index - 1, index + 1, index - width, index + width] {
                        if cells[next] == AIR && !visited[next] {
                            visited[next] = true;
                            queue.push_back(next);
                        }
                    }
                }
                assert!(
                    cells
                        .iter()
                        .enumerate()
                        .all(|(i, &c)| c != AIR || visited[i]),
                    "disconnected room, seed {seed}"
                );
            }
            assert_ne!(cave(width, height, 42).0, cave(width, height, 43).0);
        }
    }
}
