//! Bounded, asynchronous single-slot storage. Games own formats and migrations.
use atomic_write_file::AtomicWriteFile;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

pub const MAX_BYTES: usize = 1024 * 1024;
enum Request {
    Save(Vec<u8>),
    Load,
}
#[derive(Debug)]
pub enum Outcome {
    Saved,
    Loaded(Vec<u8>),
}
/// One operation at a time, with explicit backpressure. Drop finishes an accepted
/// write and joins the worker. No file work takes place on the calling thread.
pub struct Store {
    requests: Option<SyncSender<Request>>,
    results: Receiver<io::Result<Outcome>>,
    pending: bool,
    thread: Option<JoinHandle<()>>,
}
impl Store {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let (requests, input) = mpsc::sync_channel(1);
        let (output, results) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("charon-save".into())
            .spawn(move || {
                for request in input {
                    let result = match request {
                        Request::Save(bytes) => write(&path, &bytes).map(|()| Outcome::Saved),
                        Request::Load => read(&path).map(Outcome::Loaded),
                    };
                    if output.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            results,
            pending: false,
            thread: Some(thread),
        })
    }
    pub fn save(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.len() > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save exceeds byte limit",
            ));
        }
        self.request(Request::Save(bytes))
    }
    pub fn load(&mut self) -> io::Result<()> {
        self.request(Request::Load)
    }
    fn request(&mut self, request: Request) -> io::Result<()> {
        if self.pending {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "save operation in progress",
            ));
        }
        self.requests
            .as_ref()
            .expect("live store")
            .try_send(request)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.pending = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<io::Result<Outcome>> {
        if !self.pending {
            return None;
        }
        let result = match self.results.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(io::Error::other("save worker stopped")),
        };
        self.pending = false;
        Some(result)
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    // Same-directory temporary + synced contents + platform-specific replacement.
    // Never remove the previous save before a replacement is ready.
    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(bytes)?;
    file.commit()
}
fn read(path: &std::path::Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "save exceeds byte limit",
        ));
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime};
    fn await_result(store: &mut Store) -> io::Result<Outcome> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = store.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "save worker stalled");
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn replacement_backpressure_errors_and_shutdown_preserve_committed_data() {
        let dir = std::env::temp_dir().join(format!(
            "charon-save-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("slot");
        let mut store = Store::new(path.clone()).unwrap();
        store.load().unwrap();
        assert!(await_result(&mut store).is_err());
        store.save(b"first".to_vec()).unwrap();
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert!(matches!(await_result(&mut store).unwrap(), Outcome::Saved));
        store.save(b"second".to_vec()).unwrap();
        await_result(&mut store).unwrap();
        store.load().unwrap();
        assert!(matches!(await_result(&mut store).unwrap(), Outcome::Loaded(b) if b == b"second"));
        assert!(store.save(vec![0; MAX_BYTES + 1]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // Dropping an uncommitted temporary leaves the previous slot intact.
        let mut interrupted = AtomicWriteFile::open(&path).unwrap();
        interrupted.write_all(b"partial").unwrap();
        drop(interrupted);
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        store.save(b"on-shutdown".to_vec()).unwrap();
        drop(store);
        assert_eq!(std::fs::read(&path).unwrap(), b"on-shutdown");
        std::fs::write(&path, vec![0; MAX_BYTES + 1]).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
