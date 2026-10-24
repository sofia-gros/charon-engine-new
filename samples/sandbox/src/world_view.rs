//! Game-owned generation and active-window assembly. The storage plugin knows
//! neither Noita nor sand. Guard cells are temporary and never written back.
use crate::materials::{AIR, SAND, WALL};
use charon_world::{ChunkKey, Config, World};
use std::{
    collections::BTreeSet,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};
pub const SIDE: usize = 512;
pub const BASE: [i64; 4] = [-17920, -7168, 17920, 17408];
pub const MAIN_CAPTURE: [i64; 4] = [-17920, -31744, 17920, 41984];
pub const PARALLEL_PERIOD: i64 = 35840;
pub struct Session {
    pub storage: World,
    width: usize,
    height: usize,
}
impl Session {
    pub fn new(
        directory: PathBuf,
        seed: u64,
        materials: u64,
        width: usize,
        height: usize,
    ) -> io::Result<Self> {
        let config = Config {
            side: SIDE,
            resident_limit: 32,
            seed,
            generator_revision: 3,
            materials,
        };
        Ok(Self {
            storage: World::new(config, directory, move |key| generate(key, seed))?,
            width,
            height,
        })
    }
    fn needed(&self, origin: [i64; 2]) -> io::Result<BTreeSet<ChunkKey>> {
        let right = origin[0]
            .checked_add(self.width as i64 - 1)
            .ok_or_else(|| io::Error::other("region overflow"))?;
        let bottom = origin[1]
            .checked_add(self.height as i64 - 1)
            .ok_or_else(|| io::Error::other("region overflow"))?;
        let mut keys = BTreeSet::new();
        for y in origin[1].div_euclid(SIDE as i64)..=bottom.div_euclid(SIDE as i64) {
            for x in origin[0].div_euclid(SIDE as i64)..=right.div_euclid(SIDE as i64) {
                keys.insert(ChunkKey(x, y));
            }
        }
        if keys.len() > 32 {
            return Err(io::Error::other("active region exceeds resident budget"));
        }
        Ok(keys)
    }
    /// Call while the simulation is at a frozen handoff boundary. No disk waits.
    pub fn prepare(&mut self, origin: [i64; 2]) -> io::Result<Option<Vec<u8>>> {
        for event in self.storage.poll() {
            event.result?;
        }
        let needed = self.needed(origin)?;
        let old: Vec<_> = self
            .storage
            .keys()
            .filter(|k| !needed.contains(k))
            .collect();
        for key in old {
            let result = if self.storage.is_dirty(key) {
                self.storage.save(key)
            } else {
                self.storage.evict(key)
            };
            if let Err(e) = result {
                if e.kind() != io::ErrorKind::WouldBlock {
                    return Err(e);
                }
            }
        }
        for &key in &needed {
            if let Err(e) = self.storage.request(key) {
                if e.kind() != io::ErrorKind::WouldBlock {
                    return Err(e);
                }
            }
        }
        if needed.iter().any(|&k| self.storage.snapshot(k).is_none()) {
            return Ok(None);
        }
        let mut cells = vec![AIR; self.width * self.height];
        for key in needed {
            let chunk = self.storage.snapshot(key).expect("loaded");
            let [ox, oy] = key.origin(SIDE).expect("validated");
            let left = origin[0].max(ox);
            let right = (origin[0] + self.width as i64).min(ox + SIDE as i64);
            let top = origin[1].max(oy);
            let bottom = (origin[1] + self.height as i64).min(oy + SIDE as i64);
            for y in top..bottom {
                let source = ((y - oy) as usize) * SIDE + (left - ox) as usize;
                let target = ((y - origin[1]) as usize) * self.width + (left - origin[0]) as usize;
                let count = (right - left) as usize;
                cells[target..target + count].copy_from_slice(&chunk[source..source + count]);
            }
        }
        for y in 0..self.height {
            cells[y * self.width] = WALL;
            cells[y * self.width + self.width - 1] = WALL;
        }
        cells[(self.height - 1) * self.width..].fill(WALL);
        Ok(Some(cells))
    }
    /// Caller passes a completed, frozen simulation frame. Temporary side/bottom
    /// guards must never overwrite the real world. Preserve all other cell IDs.
    pub fn capture(&mut self, origin: [i64; 2], cells: &[u8]) -> io::Result<()> {
        if cells.len() != self.width * self.height {
            return Err(io::Error::other("region cell count mismatch"));
        }
        for key in self.needed(origin)? {
            let mut chunk = self
                .storage
                .snapshot(key)
                .ok_or_else(|| io::Error::other("capture chunk missing"))?
                .to_vec();
            let [ox, oy] = key.origin(SIDE).expect("validated");
            let left = (origin[0] + 1).max(ox);
            let right = (origin[0] + self.width as i64 - 1).min(ox + SIDE as i64);
            let top = origin[1].max(oy);
            let bottom = (origin[1] + self.height as i64 - 1).min(oy + SIDE as i64);
            if right <= left {
                continue;
            }
            for y in top..bottom {
                let source = ((y - origin[1]) as usize) * self.width + (left - origin[0]) as usize;
                let target = ((y - oy) as usize) * SIDE + (left - ox) as usize;
                let count = (right - left) as usize;
                chunk[target..target + count].copy_from_slice(&cells[source..source + count]);
            }
            self.storage.replace(key, &chunk)?;
        }
        Ok(())
    }
    /// Shutdown path only. Drawing/updates use prepare/poll rather than waiting.
    pub fn flush(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for event in self.storage.poll() {
                event.result?;
            }
            let dirty: Vec<_> = self
                .storage
                .keys()
                .filter(|&k| self.storage.is_dirty(k))
                .collect();
            for key in dirty.iter().copied() {
                if let Err(e) = self.storage.save(key) {
                    if e.kind() != io::ErrorKind::WouldBlock {
                        return Err(e);
                    }
                }
            }
            if dirty.is_empty() && self.storage.pending_count() == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "world save is still pending",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
/// Terraria-inspired surface strata and ores, with Noita-inspired connected
/// underground passages and wider chambers. These are independent game rules,
/// not either game's proprietary generation algorithm. Geometry uses 4px blocks.
pub fn surface_y(x: i64, seed: u64) -> i64 {
    let noise = charon_world::noise::Noise::new(seed);
    let distance = x.unsigned_abs();
    let blend = distance.saturating_sub(48).min(80) as i64;
    let variation = i64::from(noise.sample(x.div_euclid(4), 0, 96)) * 10 / 65535;
    12 + (variation * blend / 80) * 4
}
fn block_material(bx: i64, by: i64, seed: u64) -> u8 {
    use crate::materials::{DIRT, ORE, STONE};
    let n = charon_world::noise::Noise::new(seed);
    let ground = surface_y(bx * 4, seed).div_euclid(4);
    if by < ground {
        return AIR;
    }
    let depth = by - ground;
    let band = by.div_euclid(48);
    let center = band * 48 + 28 + i64::from(n.sample(bx, band, 64)) * 5 / 65535;
    let chamber = (by - center).abs() <= 12 && n.fractal(bx, by) > 14000;
    let tunnel_half = 3 + (i64::from(n.sample(bx, band.wrapping_add(13), 48)) + 65535) * 3 / 131070;
    let horizontal = depth >= 12 && (by - center).abs() <= tunnel_half;
    let column = bx.div_euclid(96);
    let shaft_center = column * 96 + 48 + i64::from(n.sample(column, by, 48)) * 3 / 65535;
    let shaft = (bx - shaft_center).abs() <= 3 && bx.unsigned_abs() > 24;
    if shaft {
        // Alternating 8px ledges, 20px apart, connect the surface and cave bands.
        let left = by.div_euclid(5).rem_euclid(2) == 0;
        let ledge = by.rem_euclid(5) == 0
            && if left {
                bx < shaft_center - 1
            } else {
                bx > shaft_center + 1
            };
        if !ledge {
            return AIR;
        }
    } else if horizontal || (depth >= 12 && chamber) {
        return AIR;
    }
    if depth < 2 && n.sample(bx, 7, 32) > 22000 && bx.unsigned_abs() > 24 {
        return SAND;
    }
    if depth > 20 && n.sample(bx, by, 6) > 40000 && n.sample(bx, by, 40) > 0 {
        return ORE;
    }
    if depth < 14 + i64::from(n.sample(bx, 3, 64)) * 5 / 65535 {
        DIRT
    } else {
        STONE
    }
}
pub fn generate(key: ChunkKey, seed: u64) -> io::Result<Vec<u8>> {
    let [ox, oy] = key
        .origin(SIDE)
        .ok_or_else(|| io::Error::other("chunk overflow"))?;
    ox.checked_add(SIDE as i64 - 1)
        .ok_or_else(|| io::Error::other("chunk overflow"))?;
    oy.checked_add(SIDE as i64 - 1)
        .ok_or_else(|| io::Error::other("chunk overflow"))?;
    let mut cells = vec![AIR; SIDE * SIDE];
    for y in (0..SIDE).step_by(4) {
        for x in (0..SIDE).step_by(4) {
            let material = block_material(
                (ox + x as i64).div_euclid(4),
                (oy + y as i64).div_euclid(4),
                seed,
            );
            for row in y..y + 4 {
                cells[row * SIDE + x..row * SIDE + x + 4].fill(material);
            }
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_runtime::Simulation;
    use charon_sand::{Command, SandWorld};
    fn region(session: &mut Session, origin: [i64; 2]) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(cells) = session.prepare(origin).unwrap() {
                return cells;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn geology_is_block_aligned_seeded_and_contains_strata_caves_and_ores() {
        use crate::materials::{DIRT, ORE, STONE};
        let a = generate(ChunkKey(0, 0), 42).unwrap();
        assert_eq!(a, generate(ChunkKey(0, 0), 42).unwrap());
        assert_ne!(a, generate(ChunkKey(0, 0), 43).unwrap());
        for material in [AIR, DIRT, STONE, ORE] {
            assert!(a.contains(&material), "missing material {material}");
        }
        for key in [ChunkKey(-1, 0), ChunkKey(0, 0), ChunkKey(1, 0)] {
            let cells = generate(key, 42).unwrap();
            let [ox, oy] = key.origin(SIDE).unwrap();
            for y in (0..SIDE).step_by(4) {
                for x in (0..SIDE).step_by(4) {
                    let expected = block_material(
                        (ox + x as i64).div_euclid(4),
                        (oy + y as i64).div_euclid(4),
                        42,
                    );
                    for dy in 0..4 {
                        assert!(cells[(y + dy) * SIDE + x..(y + dy) * SIDE + x + 4]
                            .iter()
                            .all(|&id| id == expected));
                    }
                }
            }
        }
        // Follow open cells from the surface into a generated underground band.
        let mut seen = vec![false; a.len()];
        let mut queue = std::collections::VecDeque::new();
        for x in 0..SIDE {
            if a[x] == AIR {
                seen[x] = true;
                queue.push_back(x);
            }
        }
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i % SIDE, i / SIDE);
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < SIDE && ny < SIDE {
                    let j = ny * SIDE + nx;
                    if !seen[j] && a[j] == AIR {
                        seen[j] = true;
                        queue.push_back(j);
                    }
                }
            }
        }
        assert!(
            seen[128 * SIDE..].iter().any(|&v| v),
            "surface has no open connection to the caves"
        );
    }
    #[test]
    fn moving_active_window_preserves_global_physics_and_never_saves_guard_walls() {
        let dir = std::env::temp_dir().join(format!("charon-region-{}", std::process::id()));
        let materials = crate::materials::palette();
        let mut session = Session::new(dir.clone(), 42, materials.fingerprint(), 64, 96).unwrap();
        let a = [480, -32];
        let b = [496, -32];
        let cells = region(&mut session, a);
        let mut first =
            SandWorld::from_region(64, 96, 1, cells, materials.clone(), a, 100).unwrap();
        first.step(vec![Command::Paint {
            x: 31,
            y: 15,
            material: SAND,
        }]);
        for _ in 0..8 {
            first.step(vec![]);
        }
        let clock = first.tick();
        let frozen = first.edit(vec![]).indices.to_bytes();
        session.capture(a, &frozen).unwrap();
        assert_eq!(
            session.storage.material([480, 0]),
            Some(AIR),
            "temporary left guard leaked to real terrain"
        );
        let cells = region(&mut session, b);
        let mut shifted =
            SandWorld::from_region(64, 96, 4, cells, materials.clone(), b, clock).unwrap();
        for _ in 0..10 {
            first.step(vec![]);
            shifted.step(vec![]);
            for gy in -20..12 {
                for gx in 506..519 {
                    assert_eq!(
                        first.material_at(gx - 480, gy + 32),
                        shifted.material_at(gx - 496, gy + 32),
                        "global {gx},{gy} diverged across storage seam after shift"
                    );
                }
            }
            assert_eq!(first.tick(), shifted.tick());
        }
        let final_cells = shifted.edit(vec![]).indices.to_bytes();
        session.capture(b, &final_cells).unwrap();
        session.flush().unwrap();
        drop(session);
        let mut reopened = Session::new(dir.clone(), 42, materials.fingerprint(), 64, 96).unwrap();
        assert_eq!(region(&mut reopened, b), final_cells);
        assert!(reopened.storage.resident_count() <= 32);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn noita_reference_extents_and_far_parallel_regions_do_not_allocate_the_world() {
        assert_eq!((BASE[2] - BASE[0], BASE[3] - BASE[1]), (35840, 24576));
        assert_eq!(
            (
                MAIN_CAPTURE[2] - MAIN_CAPTURE[0],
                MAIN_CAPTURE[3] - MAIN_CAPTURE[1]
            ),
            (35840, 73728)
        );
        assert_eq!(PARALLEL_PERIOD, SIDE as i64 * 70);
        let dir = std::env::temp_dir().join(format!("charon-far-region-{}", std::process::id()));
        let mut session = Session::new(
            dir.clone(),
            42,
            crate::materials::palette().fingerprint(),
            64,
            96,
        )
        .unwrap();
        let first = region(&mut session, [-32, -32]);
        for origin in [
            [PARALLEL_PERIOD * 1000 - 32, -32],
            [-PARALLEL_PERIOD * 1000 - 32, MAIN_CAPTURE[3] + 512],
            [-32, -32],
        ] {
            let cells = region(&mut session, origin);
            assert!(session.storage.resident_count() <= 8);
            assert_eq!(cells.len(), 64 * 96);
            if origin == [-32, -32] {
                assert_eq!(cells, first);
            }
        }
        assert!(!dir.exists());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::game::{Command, Game};
    use charon_runtime::Simulation;
    fn await_region(session: &mut Session, origin: [i64; 2]) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(cells) = session.prepare(origin).unwrap() {
                return cells;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn streamed_combat_despawns_outside_region_and_replays_new_encounter_on_return() {
        let root = std::env::temp_dir().join(format!("charon-world-combat-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("inputs.trace");
        let mut game = Game::new(640, 360, 4, true).unwrap();
        game.record_to(&path).unwrap();
        let mut frame = game.step(vec![Command::StartWorldGame(42)]);
        let origin = frame.stream.unwrap().origin;
        let mut session = Session::new(
            root.join("chunks"),
            42,
            game.material_fingerprint(),
            640,
            360,
        )
        .unwrap();
        let cells = await_region(&mut session, origin);
        frame = game.step(vec![Command::LoadRegion {
            origin,
            cells: cells.into(),
        }]);
        assert_eq!(frame.simulation_tick, 0);
        assert_eq!(
            frame.combat.as_ref().unwrap().phase,
            crate::combat::Phase::Ready
        );
        game.step(vec![
            Command::Begin,
            Command::Pause,
            Command::Terrain(charon_sand::Command::FillRect {
                x: 128,
                y: 176,
                width: 508,
                height: 48,
                material: AIR,
            }),
        ]);
        for _ in 0..40 {
            frame = game.step(vec![Command::Step]);
        }
        let enemy = frame.combat.as_ref().unwrap().enemies[0];
        game.step(vec![Command::FireAt([
            enemy.rect[0] + 3.5,
            enemy.rect[1] + 6.,
        ])]);
        for _ in 0..16 {
            frame = game.step(vec![Command::Step]);
        }
        assert_eq!(frame.combat.as_ref().unwrap().enemies[0].health, 2);
        frame = game.step(vec![Command::Pause, Command::Right(true)]);
        for tick in 0..260 {
            if frame.stream.unwrap().waiting {
                break;
            }
            frame = game.step(if tick % 30 == 0 {
                vec![Command::Jump]
            } else {
                vec![]
            });
        }
        assert!(
            frame.stream.unwrap().waiting,
            "player failed to reach boundary: {:?}",
            frame.player
        );
        assert_eq!(frame.combat.as_ref().unwrap().enemies[0].health, 2);
        session
            .capture(origin, &frame.terrain.indices.to_bytes())
            .unwrap();
        let next = [origin[0] + 512, origin[1]];
        let cells = await_region(&mut session, next);
        frame = game.step(vec![
            Command::Pause,
            Command::Right(false),
            Command::LoadRegion {
                origin: next,
                cells: cells.into(),
            },
        ]);
        assert!(frame.combat.as_ref().unwrap().enemies.is_empty());
        assert_eq!(frame.combat.as_ref().unwrap().despawned, 1);
        frame = game.step(vec![Command::Pause, Command::Left(true)]);
        for _ in 0..10 {
            if frame.stream.unwrap().waiting {
                break;
            }
            frame = game.step(vec![]);
        }
        assert!(frame.stream.unwrap().waiting);
        session
            .capture(next, &frame.terrain.indices.to_bytes())
            .unwrap();
        let cells = await_region(&mut session, origin);
        frame = game.step(vec![
            Command::Pause,
            Command::Left(false),
            Command::LoadRegion {
                origin,
                cells: cells.into(),
            },
        ]);
        let combat = frame.combat.as_ref().unwrap();
        let returned = combat.enemies.iter().find(|e| e.id != enemy.id).unwrap();
        assert_eq!(returned.health, 3);
        assert_eq!(returned.rect[0], enemy.rect[0]);
        assert_eq!(combat.health, 3);
        assert!(!frame.player.unwrap().embedded);
        drop(game);
        session.flush().unwrap();
        drop(session);
        assert!(crate::trace::replay(&path, 1).unwrap() > 180);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn game_handoff_freezes_edits_preserves_global_position_and_replays_without_io() {
        let root = std::env::temp_dir().join(format!("charon-world-game-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("inputs.trace");
        let mut game = Game::new(640, 360, 4, true).unwrap();
        game.record_to(&path).unwrap();
        let mut frame = game.step(vec![Command::StartWorld(42)]);
        assert_eq!(frame.state_label(), "LOADING");
        let original = frame.stream.unwrap().origin;
        for _ in 0..5 {
            frame = game.step(vec![]);
            assert_eq!(frame.simulation_tick, 0);
        }
        let before = frame.terrain.indices.to_bytes();
        let blocked = game.step(vec![Command::Terrain(charon_sand::Command::Paint {
            x: 320,
            y: 180,
            material: WALL,
        })]);
        assert_eq!(blocked.terrain.indices.to_bytes(), before);
        assert_eq!(
            blocked.image_error,
            Some("WORLD LOADING - ACTION NOT APPLIED")
        );
        let mut session = Session::new(
            root.join("chunks"),
            42,
            game.material_fingerprint(),
            640,
            360,
        )
        .unwrap();
        let cells = await_region(&mut session, original);
        frame = game.step(vec![
            Command::LoadRegion {
                origin: original,
                cells: cells.into(),
            },
            Command::Right(true),
        ]);
        for tick in 0..300 {
            if frame.stream.unwrap().waiting {
                break;
            }
            frame = game.step(if tick % 30 == 0 {
                vec![Command::Jump]
            } else {
                vec![]
            });
        }
        assert!(
            frame.stream.unwrap().waiting,
            "walking did not reach a region boundary"
        );
        let global = [
            original[0] as f32 + frame.player.unwrap().x(),
            original[1] as f32 + frame.player.unwrap().y(),
        ];
        let clock = frame.simulation_tick;
        let frozen = frame.terrain.indices.to_bytes();
        session.capture(original, &frozen).unwrap();
        for _ in 0..5 {
            assert_eq!(game.step(vec![]).simulation_tick, clock);
        }
        let target = [original[0] + 512, original[1]];
        let cells = await_region(&mut session, target);
        frame = game.step(vec![
            Command::Pause,
            Command::LoadRegion {
                origin: target,
                cells: cells.into(),
            },
        ]);
        assert_eq!(frame.simulation_tick, clock);
        assert_eq!(
            [
                target[0] as f32 + frame.player.unwrap().x(),
                target[1] as f32 + frame.player.unwrap().y()
            ],
            global
        );
        assert!(!frame.stream.unwrap().waiting);
        assert!(frame.paused);
        game.step(vec![Command::Right(false)]);
        session
            .capture(target, &frame.terrain.indices.to_bytes())
            .unwrap();
        session.flush().unwrap();
        drop(game);
        drop(session);
        let count = crate::trace::replay(&path, 1).unwrap();
        assert!(count > 170);
        std::fs::remove_dir_all(root).unwrap();
    }
}
