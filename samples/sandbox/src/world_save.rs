//! Game snapshots and immutable checkpoint directories; no format imposed on core.
use charon_platformer::Body;
use charon_projectiles::{Projectiles, Shot};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyState {
    bits: [u32; 5],
    grounded: bool,
    embedded: bool,
}
impl BodyState {
    pub fn capture(body: Body) -> Self {
        Self {
            bits: [
                body.x(),
                body.y(),
                body.width(),
                body.height(),
                body.vertical_velocity(),
            ]
            .map(f32::to_bits),
            grounded: body.grounded,
            embedded: body.embedded,
        }
    }
    pub fn restore(&self) -> Result<Body, String> {
        let [x, y, w, h, v] = self.bits.map(f32::from_bits);
        if ![x, y, w, h]
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 1_000_000.)
            || w != 7.
            || h != 12.
        {
            return Err("invalid saved body".into());
        }
        let mut body = Body::new(x, y, w, h);
        body.restore_motion(v, self.grounded, self.embedded)?;
        Ok(body)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotState {
    bits: [u32; 6],
    ticks: u16,
}
impl ShotState {
    pub fn capture(s: Shot) -> Self {
        Self {
            bits: [
                s.position[0],
                s.position[1],
                s.velocity[0],
                s.velocity[1],
                s.acceleration[0],
                s.acceleration[1],
            ]
            .map(f32::to_bits),
            ticks: s.remaining_ticks,
        }
    }
    pub fn restore(&self) -> Result<Shot, String> {
        let [x, y, vx, vy, ax, ay] = self.bits.map(f32::from_bits);
        let shot = Shot {
            position: [x, y],
            velocity: [vx, vy],
            acceleration: [ax, ay],
            remaining_ticks: self.ticks,
        };
        Projectiles::default().spawn(shot)?;
        Ok(shot)
    }
}
pub fn shots(values: &[ShotState]) -> Result<Projectiles, String> {
    if values.len() > 1024 {
        return Err("too many saved shots".into());
    }
    let mut shots = Projectiles::default();
    for value in values {
        shots.spawn(value.restore()?)?;
    }
    Ok(shots)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyState {
    pub origin: [i64; 2],
    pub active: bool,
    pub chase: bool,
    pub body: BodyState,
    pub health: u8,
    pub cooldown: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombatState {
    pub origin: [i64; 2],
    pub size: [usize; 2],
    pub phase: u8,
    pub health: u8,
    pub invulnerable: u8,
    pub kills: u32,
    pub rejected: u64,
    pub enemies: Vec<EnemyState>,
    pub shots: Vec<ShotState>,
    pub parked: Vec<([i64; 2], ShotState)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub seed: u64,
    pub materials: u64,
    pub size: [usize; 2],
    pub origin: [i64; 2],
    pub waiting: bool,
    pub tick: u64,
    pub player: BodyState,
    pub combat: CombatState,
    pub shots: Vec<ShotState>,
    pub images: Vec<[u32; 2]>,
    pub fire: bool,
    pub target: Option<[u32; 2]>,
    pub acceleration: [u32; 2],
    pub impacts: u64,
    pub facing: i8,
    pub sand_solid: bool,
    pub paused: bool,
    pub jump: bool,
}
pub fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, v| {
        (h ^ u64::from(*v)).wrapping_mul(0x100000001b3)
    })
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    version: u32,
    seed: u64,
    materials: u64,
    size: [usize; 2],
    generation: String,
    state_hash: u64,
    terrain_hash: u64,
    chunks: Vec<String>,
}
/// The previous generation is read-only. Chunk saves use atomic replacement,
/// so hard links share unchanged data without mutating the previous checkpoint.
pub struct Checkpoint {
    root: PathBuf,
    pub directory: PathBuf,
    pub saved: Option<Snapshot>,
    terrain_hash: Option<u64>,
    seed: u64,
    materials: u64,
    size: [usize; 2],
    generation: String,
}
fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn chunk_names(directory: &std::path::Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|v| v == "chunk") {
            if !entry.file_type()?.is_file() {
                return Err(invalid("checkpoint chunk is not a regular file"));
            }
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("invalid chunk filename"))?,
            );
        }
    }
    names.sort();
    Ok(names)
}
fn operation(path: PathBuf, write: Option<Vec<u8>>) -> io::Result<Vec<u8>> {
    let mut store = charon_save::Store::new(path)?;
    if let Some(bytes) = write {
        store.save(bytes)?;
    } else {
        store.load()?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(result) = store.poll() {
            return match result? {
                charon_save::Outcome::Saved => Ok(vec![]),
                charon_save::Outcome::Loaded(bytes) => Ok(bytes),
            };
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "checkpoint IO timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
impl Checkpoint {
    /// Called before the event loop; disk enumeration/copying never runs in draw.
    pub fn open(root: PathBuf, seed: u64, materials: u64, size: [usize; 2]) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        let head_path = root.join("checkpoint.json");
        let head = match operation(head_path, None) {
            Ok(bytes) => Some(serde_json::from_slice::<Head>(&bytes).map_err(invalid)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let mut saved = None;
        let mut terrain_hash = None;
        let source = if let Some(head) = head {
            if head.version != 1
                || head.seed != seed
                || head.materials != materials
                || head.size != size
                || !head.generation.starts_with('g')
                || !head.generation[1..]
                    .bytes()
                    .all(|c| c.is_ascii_digit() || c == b'-')
            {
                return Err(invalid("incompatible world checkpoint"));
            }
            let source = root.join("checkpoints").join(head.generation);
            let bytes = operation(source.join("state.json"), None)?;
            if checksum(&bytes) != head.state_hash {
                return Err(invalid("world state checksum mismatch"));
            }
            let state: Snapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
            if state.version != 1
                || state.seed != seed
                || state.materials != materials
                || state.size != size
            {
                return Err(invalid("world state identity mismatch"));
            }
            let mut actual = chunk_names(&source)?;
            actual.sort();
            let mut expected = head.chunks.clone();
            expected.sort();
            if actual != expected {
                return Err(invalid("checkpoint chunk set mismatch"));
            }
            terrain_hash = Some(head.terrain_hash);
            saved = Some(state);
            source
        } else {
            root.clone()
        }; // Import existing terrain-only saves without overwriting them.
        let generation = format!(
            "g{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(invalid)?
                .as_nanos(),
            std::process::id()
        );
        let directory = root.join("checkpoints").join(&generation);
        fs::create_dir_all(directory.parent().unwrap())?;
        fs::create_dir(&directory)?;
        for entry in fs::read_dir(&source)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|v| v == "chunk")
            {
                let target = directory.join(entry.file_name());
                if fs::hard_link(entry.path(), &target).is_err() {
                    fs::copy(entry.path(), &target)?;
                    fs::File::open(&target)?.sync_all()?;
                }
            }
        }
        Ok(Self {
            root,
            directory,
            saved,
            terrain_hash,
            seed,
            materials,
            size,
            generation,
        })
    }
    pub fn verify_terrain(&self, cells: &[u8]) -> io::Result<()> {
        if self
            .terrain_hash
            .is_some_and(|hash| hash != checksum(cells))
        {
            return Err(invalid("checkpoint terrain does not match game state"));
        }
        Ok(())
    }
    /// Call only after capturing the final region and flushing every dirty chunk.
    /// A failed state write or HEAD replacement leaves the previous HEAD usable.
    pub fn commit(&self, state: &Snapshot, cells: &[u8]) -> io::Result<()> {
        if state.seed != self.seed || state.materials != self.materials || state.size != self.size {
            return Err(invalid("checkpoint identity mismatch"));
        }
        let bytes = serde_json::to_vec(state).map_err(invalid)?;
        let hash = checksum(&bytes);
        operation(self.directory.join("state.json"), Some(bytes))?;
        let head = Head {
            version: 1,
            seed: self.seed,
            materials: self.materials,
            size: self.size,
            generation: self.generation.clone(),
            state_hash: hash,
            terrain_hash: checksum(cells),
            chunks: chunk_names(&self.directory)?,
        };
        operation(
            self.root.join("checkpoint.json"),
            Some(serde_json::to_vec(&head).map_err(invalid)?),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::{Command, Game},
        world_view::Session,
    };
    use charon_runtime::Simulation;
    fn region(session: &mut Session, origin: [i64; 2]) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(cells) = session.prepare(origin).unwrap() {
                return cells;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "charon-checkpoint-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn checkpoint_roundtrip_preserves_live_attacks_and_future_simulation_and_survives_failed_publication(
    ) {
        let root = root("roundtrip");
        let mut game = Game::new(640, 360, 4, true).unwrap();
        let materials = game.material_fingerprint();
        let checkpoint = Checkpoint::open(root.clone(), 42, materials, [640, 360]).unwrap();
        let mut session =
            Session::new(checkpoint.directory.clone(), 42, materials, 640, 360).unwrap();
        let start = game.step(vec![Command::StartWorldGame(42)]);
        let origin = start.stream.unwrap().origin;
        let cells = region(&mut session, origin);
        game.step(vec![
            Command::LoadRegion {
                origin,
                cells: cells.into(),
            },
            Command::Begin,
            Command::Pause,
            Command::FireAt([143.5, 186.]),
        ]);
        for _ in 0..16 {
            game.step(vec![Command::Step]);
        }
        game.step(vec![
            Command::PlaceImage([340.25, 150.5]),
            Command::LobAt([500., 170.]),
        ]);
        let frame = game.step(vec![Command::Step]);
        assert_eq!(frame.combat.as_ref().unwrap().enemies[0].health, 2);
        assert!(!frame.shots.is_empty());
        let state = frame.checkpoint.unwrap();
        let cells = frame.terrain.indices.to_bytes();
        let signature = game.signature();
        session.capture(origin, &cells).unwrap();
        session.flush().unwrap();
        checkpoint.commit(&state, &cells).unwrap();
        drop(session);
        let head = fs::read(root.join("checkpoint.json")).unwrap();
        let next = Checkpoint::open(root.clone(), 42, materials, [640, 360]).unwrap();
        let mut session = Session::new(next.directory.clone(), 42, materials, 640, 360).unwrap();
        let restored_cells = region(&mut session, origin);
        next.verify_terrain(&restored_cells).unwrap();
        assert_eq!(restored_cells, cells);
        let mut restored = Game::new(640, 360, 1, true).unwrap();
        let trace = root.join("restore.trace");
        restored.record_to(&trace).unwrap();
        restored.step(vec![Command::RestoreCheckpoint {
            state: std::sync::Arc::new(next.saved.clone().unwrap()),
            cells: restored_cells.into(),
        }]);
        assert_eq!(restored.signature(), signature);
        for _ in 0..40 {
            game.step(vec![Command::Step]);
            restored.step(vec![Command::Step]);
            assert_eq!(
                restored.signature(),
                game.signature(),
                "resumed simulation diverged"
            );
        }
        drop(restored);
        assert_eq!(crate::trace::replay(&trace, 2).unwrap(), 41);
        // Abandon a modified working generation: a restart must still load the
        // previous complete terrain and metadata, not this partial save.
        let mut changed = cells.clone();
        changed[220 * 640 + 320] = crate::materials::AIR;
        assert_ne!(changed, cells);
        session.capture(origin, &changed).unwrap();
        session.flush().unwrap();
        fs::create_dir(next.directory.join("state.json")).unwrap();
        assert!(next.commit(&state, &changed).is_err());
        assert_eq!(fs::read(root.join("checkpoint.json")).unwrap(), head);
        drop(session);
        let previous = Checkpoint::open(root.clone(), 42, materials, [640, 360]).unwrap();
        let mut previous_session =
            Session::new(previous.directory.clone(), 42, materials, 640, 360).unwrap();
        let previous_cells = region(&mut previous_session, origin);
        previous.verify_terrain(&previous_cells).unwrap();
        assert_eq!(previous_cells, cells);
        drop(previous_session);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn corrupt_incompatible_or_missing_checkpoint_data_is_rejected_without_replacement() {
        let root = root("corrupt");
        let mut game = Game::new(640, 360, 1, true).unwrap();
        let materials = game.material_fingerprint();
        let checkpoint = Checkpoint::open(root.clone(), 42, materials, [640, 360]).unwrap();
        let mut session =
            Session::new(checkpoint.directory.clone(), 42, materials, 640, 360).unwrap();
        let origin = game
            .step(vec![Command::StartWorldGame(42)])
            .stream
            .unwrap()
            .origin;
        let cells = region(&mut session, origin);
        game.step(vec![Command::LoadRegion {
            origin,
            cells: cells.into(),
        }]);
        let frame = game.step(vec![
            Command::Begin,
            Command::Pause,
            Command::Terrain(charon_sand::Command::Paint {
                x: 330,
                y: 180,
                material: crate::materials::WALL,
            }),
        ]);
        let cells = frame.terrain.indices.to_bytes();
        session.capture(origin, &cells).unwrap();
        session.flush().unwrap();
        checkpoint
            .commit(&frame.checkpoint.unwrap(), &cells)
            .unwrap();
        drop(session);
        let head = fs::read(root.join("checkpoint.json")).unwrap();
        assert!(Checkpoint::open(root.clone(), 43, materials, [640, 360]).is_err());
        assert!(Checkpoint::open(root.clone(), 42, materials ^ 1, [640, 360]).is_err());
        let state_path = checkpoint.directory.join("state.json");
        let state = fs::read(&state_path).unwrap();
        fs::write(&state_path, b"{}").unwrap();
        assert!(Checkpoint::open(root.clone(), 42, materials, [640, 360]).is_err());
        fs::write(&state_path, state).unwrap();
        // Ensure a committed chunk exists even when the generated frame was static.
        let names = chunk_names(&checkpoint.directory).unwrap();
        assert!(!names.is_empty());
        fs::remove_file(checkpoint.directory.join(&names[0])).unwrap();
        assert!(Checkpoint::open(root.clone(), 42, materials, [640, 360]).is_err());
        assert_eq!(fs::read(root.join("checkpoint.json")).unwrap(), head);
        fs::remove_dir_all(root).unwrap();
    }
}
