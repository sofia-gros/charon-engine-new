//! Transactional, grid-aligned kinematic boundaries. Geometry and movement
//! policy belong to games; this component only couples boundaries to liquid.
use super::{Motion, SandWorld};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObstacleId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObstacleError {
    InvalidShape,
    Missing,
    Blocked,
    NoRoom,
    SearchBudget,
}
pub(super) struct Obstacle {
    cells: Vec<usize>,
    material: u8,
}
#[derive(Default)]
pub(super) struct Obstacles {
    bodies: BTreeMap<ObstacleId, Obstacle>,
    owners: HashMap<usize, ObstacleId>,
    next: u64,
    scratch: Scratch,
}
/// Reused flat work arrays keep pressure searches independent of allocator/hash
/// costs. Only the changed indices are committed; failed searches touch no world.
#[derive(Default)]
struct Scratch {
    cells: Vec<u8>,
    seen: Vec<u32>,
    parents: Vec<usize>,
    generation: u32,
    queue: Vec<usize>,
    path: Vec<usize>,
    changed: Vec<usize>,
}
impl Scratch {
    fn set(&mut self, index: usize, value: u8) {
        if self.cells[index] != value {
            self.cells[index] = value;
            self.changed.push(index);
        }
    }
}
impl Obstacles {
    pub fn clear(&mut self) {
        self.bodies.clear();
        self.owners.clear();
    }
    pub fn contains(&self, index: usize) -> bool {
        self.owners.contains_key(&index)
    }
}
impl SandWorld {
    /// Register an absolute cell mask in empty space. Empty/duplicate/outside
    /// masks are rejected atomically; no terrain or liquid is overwritten.
    pub fn add_obstacle(
        &mut self,
        points: &[[i32; 2]],
        material: u8,
    ) -> Result<ObstacleId, ObstacleError> {
        if points.is_empty()
            || points.len() > 16384
            || self
                .materials
                .get(material)
                .is_none_or(|m| m.motion != Motion::Static)
        {
            return Err(ObstacleError::InvalidShape);
        }
        let cells: BTreeSet<_> = points
            .iter()
            .map(|&[x, y]| self.obstacle_index(x, y))
            .collect::<Result<_, _>>()?;
        if cells.len() != points.len() {
            return Err(ObstacleError::InvalidShape);
        }
        if cells
            .iter()
            .any(|&i| self.cells[i] != self.materials.air() || self.obstacles.contains(i))
        {
            return Err(ObstacleError::Blocked);
        }
        self.obstacles.next = self
            .obstacles
            .next
            .checked_add(1)
            .ok_or(ObstacleError::InvalidShape)?;
        let id = ObstacleId(self.obstacles.next);
        for &i in &cells {
            self.counts[self.materials.air() as usize] -= 1;
            self.counts[material as usize] += 1;
            self.cells[i] = material;
            self.obstacles.owners.insert(i, id);
            self.wake(i);
        }
        self.obstacles.bodies.insert(
            id,
            Obstacle {
                cells: cells.into_iter().collect(),
                material,
            },
        );
        Ok(id)
    }
    fn obstacle_index(&self, x: i32, y: i32) -> Result<usize, ObstacleError> {
        if x <= 0 || y < 0 || x as usize >= self.width - 1 || y as usize >= self.height - 1 {
            return Err(ObstacleError::Blocked);
        }
        Ok(y as usize * self.width + x as usize)
    }
    /// Sweep one orthogonal cell. The caller controls speed and may repeat this
    /// operation; large jumps are rejected to prevent tunneling. Liquid is pushed
    /// along connected paths, preserving each material's count and order on a path.
    /// If blocked/full/over budget, the entire move is left unapplied.
    pub fn move_obstacle(&mut self, id: ObstacleId, delta: [i32; 2]) -> Result<(), ObstacleError> {
        let mut scratch = std::mem::take(&mut self.obstacles.scratch);
        let result = self.move_obstacle_in(id, delta, &mut scratch);
        self.obstacles.scratch = scratch;
        result
    }
    fn move_obstacle_in(
        &mut self,
        id: ObstacleId,
        delta: [i32; 2],
        scratch: &mut Scratch,
    ) -> Result<(), ObstacleError> {
        if !matches!(delta, [1, 0] | [-1, 0] | [0, 1] | [0, -1]) {
            return Err(ObstacleError::InvalidShape);
        }
        let body = self
            .obstacles
            .bodies
            .get(&id)
            .ok_or(ObstacleError::Missing)?;
        let old = body.cells.clone();
        let material = body.material;
        let new: Vec<_> = old
            .iter()
            .map(|&i| {
                self.obstacle_index(
                    (i % self.width) as i32 + delta[0],
                    (i / self.width) as i32 + delta[1],
                )
            })
            .collect::<Result<_, _>>()?;
        scratch.cells.clone_from(&self.cells);
        scratch.seen.resize(self.cells.len(), 0);
        scratch.parents.resize(self.cells.len(), 0);
        scratch.changed.clear();
        for &index in &old {
            scratch.set(index, self.materials.air());
        }
        let mut displaced = Vec::new();
        for &i in &new {
            if self
                .obstacles
                .owners
                .get(&i)
                .is_some_and(|&owner| owner != id)
            {
                return Err(ObstacleError::Blocked);
            }
            let cell = scratch.cells[i];
            match self.materials.get(cell).unwrap().motion {
                Motion::Empty => {}
                Motion::Liquid => displaced.push((i, cell)),
                _ => return Err(ObstacleError::Blocked),
            }
            scratch.set(i, material);
        }
        // One budget for the complete transaction, not one unbounded search per
        // displaced cell. A rejected move never silently drops physical matter.
        let mut budget = 262144_usize;
        for (source, liquid) in displaced {
            let seed = self.obstacle_index(
                (source % self.width) as i32 + delta[0],
                (source / self.width) as i32 + delta[1],
            )?;
            // A translating boundary often releases space across the same
            // connected liquid column. Transfer volume there before choosing a
            // nearby free surface: otherwise the released space becomes a deep
            // air pocket. Equal-material runs need no intermediate writes.
            scratch.path.clear();
            let mut cursor = seed;
            let mut released_space = false;
            loop {
                budget = budget.checked_sub(1).ok_or(ObstacleError::SearchBudget)?;
                scratch.path.push(cursor);
                let cell = scratch.cells[cursor];
                if cell == self.materials.air() {
                    released_space = self.obstacles.owners.get(&cursor) == Some(&id);
                    break;
                }
                if !self.is_liquid(cell) {
                    break;
                }
                let Ok(next) = self.obstacle_index(
                    (cursor % self.width) as i32 + delta[0],
                    (cursor / self.width) as i32 + delta[1],
                ) else {
                    break;
                };
                cursor = next;
            }
            if released_space {
                let mut carry = liquid;
                for position in 0..scratch.path.len() {
                    let index = scratch.path[position];
                    let cell = scratch.cells[index];
                    scratch.set(index, carry);
                    carry = cell;
                }
                debug_assert_eq!(carry, self.materials.air());
                continue;
            }
            scratch.generation = scratch.generation.wrapping_add(1);
            if scratch.generation == 0 {
                scratch.seen.fill(0);
                scratch.generation = 1;
            }
            let generation = scratch.generation;
            scratch.queue.clear();
            scratch.queue.push(seed);
            scratch.parents[seed] = seed;
            scratch.seen[seed] = generation;
            let mut head = 0;
            let mut end = None;
            while head < scratch.queue.len() {
                let index = scratch.queue[head];
                head += 1;
                budget = budget.checked_sub(1).ok_or(ObstacleError::SearchBudget)?;
                let cell = scratch.cells[index];
                if cell == self.materials.air() {
                    end = Some(index);
                    break;
                }
                if !self.is_liquid(cell) {
                    continue;
                }
                let x = (index % self.width) as i32;
                let y = (index / self.width) as i32;
                for [dx, dy] in [
                    delta,
                    [0, -1],
                    [0, 1],
                    [-delta[0], -delta[1]],
                    [-1, 0],
                    [1, 0],
                ] {
                    if let Ok(next) = self.obstacle_index(x + dx, y + dy) {
                        if scratch.seen[next] != generation {
                            scratch.seen[next] = generation;
                            scratch.parents[next] = index;
                            scratch.queue.push(next);
                        }
                    }
                }
            }
            let mut index = end.ok_or(ObstacleError::NoRoom)?;
            scratch.path.clear();
            scratch.path.push(index);
            while index != seed {
                index = scratch.parents[index];
                scratch.path.push(index);
            }
            let mut carry = liquid;
            for i in (0..scratch.path.len()).rev() {
                let index = scratch.path[i];
                let cell = scratch.cells[index];
                scratch.set(index, carry);
                carry = cell;
            }
            debug_assert_eq!(carry, self.materials.air());
        }
        for &index in &old {
            self.obstacles.owners.remove(&index);
        }
        for &index in &new {
            self.obstacles.owners.insert(index, id);
        }
        self.obstacles.bodies.get_mut(&id).unwrap().cells = new;
        scratch.changed.sort_unstable();
        scratch.changed.dedup();
        scratch
            .changed
            .retain(|&index| self.cells[index] != scratch.cells[index]);
        for &index in &scratch.changed {
            let cell = scratch.cells[index];
            if self.cells[index] != cell {
                let row = index / self.width;
                self.liquid_rows[row] -= usize::from(self.is_liquid(self.cells[index]));
                self.liquid_rows[row] += usize::from(self.is_liquid(cell));
                self.cells[index] = cell;
            }
        }
        for &index in &scratch.changed {
            self.wake(index);
        }
        Ok(())
    }
    pub fn remove_obstacle(&mut self, id: ObstacleId) -> Result<(), ObstacleError> {
        let body = self
            .obstacles
            .bodies
            .remove(&id)
            .ok_or(ObstacleError::Missing)?;
        for index in body.cells {
            self.obstacles.owners.remove(&index);
            self.counts[body.material as usize] -= 1;
            self.counts[self.materials.air() as usize] += 1;
            self.cells[index] = self.materials.air();
            self.wake(index);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Material, Materials, Simulation, WALL};
    fn world(workers: usize) -> SandWorld {
        let base = Materials::default();
        let mut entries: Vec<_> = (0..3).map(|id| base.get(id).unwrap().clone()).collect();
        for (key, density) in [("water", 1000), ("oil", 800)] {
            entries.push(Material {
                key: key.into(),
                color: [30, 140, 220, 255],
                motion: Motion::Liquid,
                density,
                blocks_actors: false,
                blocks_projectiles: false,
            });
        }
        SandWorld::with_materials(65, 66, workers, Materials::new(entries, 0, 2).unwrap()).unwrap()
    }
    fn cup() -> Vec<[i32; 2]> {
        (0..24)
            .flat_map(|y| {
                (0..24)
                    .filter_map(move |x| (x == 0 || x == 23 || y == 23).then_some([x + 20, y + 30]))
            })
            .collect()
    }
    #[test]
    fn moving_cup_preserves_species_cannot_be_erased_and_rolls_back_blocked_moves() {
        let mut world = world(4);
        let id = world.add_obstacle(&cup(), WALL).unwrap();
        world.edit(vec![
            Command::FillRect {
                x: 21,
                y: 41,
                width: 22,
                height: 12,
                material: 3,
            },
            Command::FillRect {
                x: 21,
                y: 37,
                width: 22,
                height: 4,
                material: 4,
            },
        ]);
        let counts = |w: &SandWorld| [3, 4].map(|id| w.cells.iter().filter(|&&c| c == id).count());
        let initial = counts(&world);
        let mut offset = 0;
        for turn in 0..4 {
            for _ in 0..8 {
                let direction = if turn % 2 == 0 { 1 } else { -1 };
                world.move_obstacle(id, [direction, 0]).unwrap();
                offset += direction;
                let assert_filled = |world: &SandWorld| {
                    for y in 37..53 {
                        for x in 21 + offset..43 + offset {
                            assert_eq!(
                                world.material_at(x, y),
                                Some(if y < 41 { 4 } else { 3 }),
                                "moving boundary left a gap or mixed layers at ({x}, {y})"
                            );
                        }
                    }
                };
                assert_filled(&world);
                world.step(vec![]);
                assert_filled(&world);
                assert_eq!(counts(&world), initial);
                assert_eq!([3, 4].map(|id| world.material_count(id)), initial);
                assert_eq!(world.liquid_rows.iter().sum::<usize>(), world.liquid);
            }
        }
        let checksum = world.checksum();
        assert_eq!(
            world.move_obstacle(id, [20, 0]),
            Err(ObstacleError::InvalidShape)
        );
        assert_eq!(world.checksum(), checksum);
        world.edit(vec![Command::FillRect {
            x: 20,
            y: 30,
            width: 1,
            height: 24,
            material: 0,
        }]);
        assert_eq!(world.material_at(20, 40), Some(WALL));
        world.edit(vec![Command::FillRect {
            x: 44,
            y: 30,
            width: 1,
            height: 24,
            material: WALL,
        }]);
        let checksum = world.checksum();
        assert_eq!(world.move_obstacle(id, [1, 0]), Err(ObstacleError::Blocked));
        assert_eq!(world.checksum(), checksum);
        let after_edit = counts(&world);
        world.remove_obstacle(id).unwrap();
        assert_eq!(counts(&world), after_edit);
        assert_eq!(world.move_obstacle(id, [1, 0]), Err(ObstacleError::Missing));
    }
    #[test]
    fn full_cavity_rejects_push_and_old_handles_do_not_survive_reset() {
        let mut world = world(1);
        world.edit(vec![
            Command::FillRect {
                x: 21,
                y: 28,
                width: 24,
                height: 1,
                material: WALL,
            },
            Command::FillRect {
                x: 21,
                y: 53,
                width: 24,
                height: 1,
                material: WALL,
            },
            Command::FillRect {
                x: 44,
                y: 28,
                width: 1,
                height: 26,
                material: WALL,
            },
            Command::FillRect {
                x: 21,
                y: 29,
                width: 23,
                height: 24,
                material: 3,
            },
        ]);
        let points: Vec<_> = (29..53).map(|y| [20, y]).collect();
        let id = world.add_obstacle(&points, WALL).unwrap();
        let checksum = world.checksum();
        assert_eq!(world.move_obstacle(id, [1, 0]), Err(ObstacleError::NoRoom));
        assert_eq!(world.checksum(), checksum);
        world.edit(vec![Command::Clear]);
        let next = world.add_obstacle(&points, WALL).unwrap();
        assert_ne!(next, id);
        assert_eq!(world.move_obstacle(id, [1, 0]), Err(ObstacleError::Missing));
    }
}
