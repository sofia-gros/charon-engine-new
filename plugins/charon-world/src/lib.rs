//! Bounded, asynchronous material-chunk storage. The game owns generation and
//! world extent; this plugin does no simulation and never allocates the world.
use atomic_write_file::AtomicWriteFile;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
};
const IN_FLIGHT: usize = 8;
const HEADER: usize = 56;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkKey(pub i64, pub i64);
impl ChunkKey {
    pub fn origin(self, side: usize) -> Option<[i64; 2]> {
        Some([
            self.0.checked_mul(side as i64)?,
            self.1.checked_mul(side as i64)?,
        ])
    }
}
#[derive(Clone, Copy)]
pub struct Config {
    pub side: usize,
    pub resident_limit: usize,
    pub seed: u64,
    pub generator_revision: u32,
    pub materials: u64,
}
struct Chunk {
    cells: Arc<[u8]>,
    revision: u64,
    saved_revision: u64,
}
enum Job {
    Load(ChunkKey),
    Save(ChunkKey, u64, Arc<[u8]>),
}
#[derive(Debug)]
pub enum Completed {
    Loaded,
    Saved(u64),
}
#[derive(Debug)]
pub struct Event {
    pub key: ChunkKey,
    pub result: io::Result<Completed>,
}
enum Response {
    Loaded(ChunkKey, io::Result<Arc<[u8]>>),
    Saved(ChunkKey, u64, io::Result<()>),
}
#[derive(Clone, Copy)]
enum Pending {
    Load,
    Save,
}
/// All filesystem and generation work runs on one worker. Loads reserve resident
/// capacity before dispatch. Saving snapshots does not block further edits;
/// acknowledgements only clean the exact saved revision. Dirty chunks cannot be
/// evicted. Drop completes accepted jobs and joins the worker.
pub struct World {
    config: Config,
    chunks: BTreeMap<ChunkKey, Chunk>,
    pending: BTreeMap<ChunkKey, Pending>,
    sender: Option<SyncSender<Job>>,
    results: Receiver<Response>,
    worker: Option<JoinHandle<()>>,
}
impl World {
    pub fn new(
        config: Config,
        directory: PathBuf,
        generator: impl Fn(ChunkKey) -> io::Result<Vec<u8>> + Send + 'static,
    ) -> io::Result<Self> {
        if !(8..=1024).contains(&config.side) || !(1..=1024).contains(&config.resident_limit) {
            return Err(invalid(
                "chunk side must be 8..1024 and resident limit 1..1024",
            ));
        }
        let (sender, jobs) = mpsc::sync_channel(IN_FLIGHT);
        let (output, results) = mpsc::sync_channel(IN_FLIGHT);
        let worker = thread::Builder::new()
            .name("charon-world-io".into())
            .spawn(move || {
                for job in jobs {
                    let response = match job {
                        Job::Load(key) => {
                            Response::Loaded(key, load(&directory, config, key, &generator))
                        }
                        Job::Save(key, revision, cells) => {
                            Response::Saved(key, revision, save(&directory, config, key, &cells))
                        }
                    };
                    if output.send(response).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            config,
            chunks: BTreeMap::new(),
            pending: BTreeMap::new(),
            sender: Some(sender),
            results,
            worker: Some(worker),
        })
    }
    /// Euclidean division is required: cell -1 belongs to chunk -1, local side-1.
    pub fn locate(&self, cell: [i64; 2]) -> (ChunkKey, usize) {
        let side = self.config.side as i64;
        (
            ChunkKey(cell[0].div_euclid(side), cell[1].div_euclid(side)),
            (cell[1].rem_euclid(side) * side + cell[0].rem_euclid(side)) as usize,
        )
    }
    pub fn resident_count(&self) -> usize {
        self.chunks.len()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn keys(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        self.chunks.keys().copied()
    }
    pub fn snapshot(&self, key: ChunkKey) -> Option<Arc<[u8]>> {
        self.chunks.get(&key).map(|c| c.cells.clone())
    }
    pub fn material(&self, cell: [i64; 2]) -> Option<u8> {
        let (key, index) = self.locate(cell);
        self.chunks.get(&key).map(|c| c.cells[index])
    }
    pub fn edit(&mut self, cell: [i64; 2], material: u8) -> io::Result<()> {
        let (key, index) = self.locate(cell);
        let chunk = self
            .chunks
            .get_mut(&key)
            .ok_or_else(|| busy("chunk is not loaded"))?;
        if chunk.cells[index] != material {
            let next = chunk
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("chunk revision overflow"))?;
            Arc::make_mut(&mut chunk.cells)[index] = material;
            chunk.revision = next;
        }
        Ok(())
    }
    pub fn is_dirty(&self, key: ChunkKey) -> bool {
        self.chunks
            .get(&key)
            .is_some_and(|c| c.revision != c.saved_revision)
    }
    pub fn request(&mut self, key: ChunkKey) -> io::Result<()> {
        if self.chunks.contains_key(&key) || self.pending.contains_key(&key) {
            return Ok(());
        }
        key.origin(self.config.side)
            .ok_or_else(|| invalid("chunk coordinate overflow"))?;
        let loading = self
            .pending
            .values()
            .filter(|p| matches!(p, Pending::Load))
            .count();
        if self.chunks.len() + loading >= self.config.resident_limit {
            return Err(busy("resident chunk limit"));
        }
        self.send(key, Job::Load(key), Pending::Load)
    }
    pub fn save(&mut self, key: ChunkKey) -> io::Result<()> {
        if self.pending.contains_key(&key) {
            return Err(busy("chunk IO is pending"));
        }
        let chunk = self
            .chunks
            .get(&key)
            .ok_or_else(|| busy("chunk is not loaded"))?;
        if !self.is_dirty(key) {
            return Ok(());
        }
        self.send(
            key,
            Job::Save(key, chunk.revision, chunk.cells.clone()),
            Pending::Save,
        )
    }
    pub fn evict(&mut self, key: ChunkKey) -> io::Result<()> {
        if self.pending.contains_key(&key) || self.is_dirty(key) {
            return Err(busy("save modified chunk successfully before eviction"));
        }
        self.chunks.remove(&key);
        Ok(())
    }
    fn send(&mut self, key: ChunkKey, job: Job, kind: Pending) -> io::Result<()> {
        if self.pending.len() >= IN_FLIGHT {
            return Err(busy("world IO queue is full"));
        }
        self.sender
            .as_ref()
            .expect("live world")
            .try_send(job)
            .map_err(|e| match e {
                TrySendError::Full(_) => busy("world IO queue is full"),
                TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "world IO worker stopped")
                }
            })?;
        self.pending.insert(key, kind);
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let response = match self.results.try_recv() {
                Ok(response) => response,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    for (key, _) in std::mem::take(&mut self.pending) {
                        events.push(Event {
                            key,
                            result: Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "world IO worker stopped",
                            )),
                        });
                    }
                    break;
                }
            };
            let (key, result) = match response {
                Response::Loaded(key, result) => (
                    key,
                    result.map(|cells| {
                        self.chunks.insert(
                            key,
                            Chunk {
                                cells,
                                revision: 0,
                                saved_revision: 0,
                            },
                        );
                        Completed::Loaded
                    }),
                ),
                Response::Saved(key, revision, result) => (
                    key,
                    result.map(|()| {
                        if let Some(chunk) = self.chunks.get_mut(&key) {
                            chunk.saved_revision = revision;
                        }
                        Completed::Saved(revision)
                    }),
                ),
            };
            self.pending.remove(&key);
            events.push(Event { key, result });
        }
        events
    }
}
impl Drop for World {
    fn drop(&mut self) {
        self.sender.take();
        // At most eight jobs total, so eight results fit even if not polled.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn busy(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}
fn path(directory: &Path, key: ChunkKey) -> PathBuf {
    directory.join(format!("{}_{}.chunk", key.0, key.1))
}
fn checksum(cells: &[u8]) -> u64 {
    cells.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn header(config: Config, key: ChunkKey, cells: &[u8]) -> Vec<u8> {
    let mut bytes = b"CHRCHNK1".to_vec();
    bytes.extend((config.side as u32).to_le_bytes());
    bytes.extend(config.generator_revision.to_le_bytes());
    bytes.extend(config.seed.to_le_bytes());
    bytes.extend(config.materials.to_le_bytes());
    bytes.extend(key.0.to_le_bytes());
    bytes.extend(key.1.to_le_bytes());
    bytes.extend(checksum(cells).to_le_bytes());
    bytes
}
fn load(
    directory: &Path,
    config: Config,
    key: ChunkKey,
    generate: &impl Fn(ChunkKey) -> io::Result<Vec<u8>>,
) -> io::Result<Arc<[u8]>> {
    let count = config.side * config.side;
    match File::open(path(directory, key)) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take((HEADER + count + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() != HEADER + count
                || bytes[..HEADER] != header(config, key, &bytes[HEADER..])
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid chunk version, identity, length or checksum",
                ));
            }
            Ok(Arc::from(&bytes[HEADER..]))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let cells = generate(key)?;
            if cells.len() != count {
                return Err(invalid("generator returned wrong cell count"));
            }
            Ok(cells.into())
        }
        Err(error) => Err(error),
    }
}
fn save(directory: &Path, config: Config, key: ChunkKey, cells: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut file = AtomicWriteFile::open(path(directory, key))?;
    file.write_all(&header(config, key, cells))?;
    file.write_all(cells)?;
    file.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    fn config(limit: usize) -> Config {
        Config {
            side: 512,
            resident_limit: limit,
            seed: 42,
            generator_revision: 1,
            materials: 99,
        }
    }
    fn directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "charon-world-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn generate(key: ChunkKey) -> io::Result<Vec<u8>> {
        let [x, y] = key.origin(512).unwrap();
        Ok((0..512 * 512)
            .map(|i| (x + i as i64 % 512 + y + i as i64 / 512).rem_euclid(7) as u8)
            .collect())
    }
    fn drain(world: &mut World) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut events = vec![];
        while world.pending_count() != 0 {
            events.extend(world.poll());
            assert!(Instant::now() < deadline, "world worker stalled");
            thread::sleep(Duration::from_millis(1));
        }
        events
    }
    fn ready(world: &mut World) {
        for event in drain(world) {
            event.result.unwrap();
        }
    }
    #[test]
    fn negative_coordinates_far_worlds_and_load_order_stay_bounded() {
        let dir = directory("coordinates");
        let mut world = World::new(config(2), dir.clone(), generate).unwrap();
        assert_eq!(world.locate([-1, -513]), (ChunkKey(-1, -2), 512 * 512 - 1));
        assert_eq!(world.locate([512, -512]), (ChunkKey(1, -1), 0));
        world.request(ChunkKey(-1, -2)).unwrap();
        world.request(ChunkKey(1_000_000, -1_000_000)).unwrap();
        assert_eq!(
            world.request(ChunkKey(0, 0)).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        ready(&mut world);
        assert_eq!(world.resident_count(), 2);
        assert_eq!(
            world.material([-1, -513]),
            Some((-514_i64).rem_euclid(7) as u8)
        );
        let first = world.snapshot(ChunkKey(-1, -2)).unwrap();
        world.evict(ChunkKey(-1, -2)).unwrap();
        world.request(ChunkKey(0, 0)).unwrap();
        ready(&mut world);
        world.evict(ChunkKey(0, 0)).unwrap();
        world.request(ChunkKey(-1, -2)).unwrap();
        ready(&mut world);
        assert_eq!(world.snapshot(ChunkKey(-1, -2)).unwrap(), first);
        assert!(
            !dir.exists(),
            "reading procedural chunks must not write to disk"
        );
    }
    #[test]
    fn immutable_snapshots_revision_ack_and_reload_preserve_edits() {
        let dir = directory("edits");
        let key = ChunkKey(-1, 0);
        let mut world = World::new(config(1), dir.clone(), generate).unwrap();
        world.request(key).unwrap();
        ready(&mut world);
        let old = world.snapshot(key).unwrap();
        let index = world.locate([-1, 0]).1;
        world.edit([-1, 0], 200).unwrap();
        assert_ne!(old[index], 200);
        assert_eq!(
            world.evict(key).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        world.save(key).unwrap();
        world.edit([-1, 0], 201).unwrap();
        ready(&mut world);
        assert!(
            world.is_dirty(key),
            "old acknowledgement cleaned a newer edit"
        );
        world.save(key).unwrap();
        ready(&mut world);
        assert!(!world.is_dirty(key));
        world.evict(key).unwrap();
        world.request(key).unwrap();
        ready(&mut world);
        assert_eq!(world.material([-1, 0]), Some(201));
        drop(world);
        let mut reopened = World::new(config(1), dir.clone(), |_| {
            panic!("saved chunk was regenerated")
        })
        .unwrap();
        reopened.request(key).unwrap();
        ready(&mut reopened);
        assert_eq!(reopened.material([-1, 0]), Some(201));
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn corrupt_or_incompatible_chunks_fail_without_regeneration_or_overwrite() {
        let dir = directory("corrupt");
        let key = ChunkKey(0, 0);
        let mut world = World::new(config(1), dir.clone(), generate).unwrap();
        world.request(key).unwrap();
        ready(&mut world);
        world.edit([0, 0], 100).unwrap();
        world.save(key).unwrap();
        ready(&mut world);
        drop(world);
        let original = std::fs::read(path(&dir, key)).unwrap();
        for mismatch in 0..4 {
            let mut c = config(1);
            match mismatch {
                0 => c.seed += 1,
                1 => c.generator_revision += 1,
                2 => c.materials += 1,
                _ => c.side = 256,
            }
            let mut world = World::new(c, dir.clone(), |_| panic!("bad file regenerated")).unwrap();
            world.request(key).unwrap();
            let events = drain(&mut world);
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].result.as_ref().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(world.resident_count(), 0);
        }
        assert_eq!(std::fs::read(path(&dir, key)).unwrap(), original);
        let mut corrupt = original;
        corrupt[HEADER + 7] ^= 1;
        std::fs::write(path(&dir, key), &corrupt).unwrap();
        let mut world = World::new(config(1), dir.clone(), |_| {
            panic!("corrupt file regenerated")
        })
        .unwrap();
        world.request(key).unwrap();
        assert_eq!(
            drain(&mut world)[0].result.as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(path(&dir, key)).unwrap(), corrupt);
        drop(world);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_write_keeps_dirty_data_and_shutdown_finishes_accepted_save() {
        let dir = directory("write-failure");
        let key = ChunkKey(0, 0);
        let mut world = World::new(config(1), dir.clone(), generate).unwrap();
        world.request(key).unwrap();
        ready(&mut world);
        world.edit([10, 10], 200).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();
        world.save(key).unwrap();
        assert!(drain(&mut world)[0].result.is_err());
        assert!(world.is_dirty(key));
        assert_eq!(world.material([10, 10]), Some(200));
        assert!(world.evict(key).is_err());
        std::fs::remove_file(&dir).unwrap();
        world.save(key).unwrap();
        drop(world);
        let mut world =
            World::new(config(1), dir.clone(), |_| panic!("shutdown lost save")).unwrap();
        world.request(key).unwrap();
        ready(&mut world);
        assert_eq!(world.material([10, 10]), Some(200));
        drop(world);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
