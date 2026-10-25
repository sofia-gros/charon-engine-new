//! Bounded ring-buffer of grid snapshots so the user can scrub back through
//! the simulation.
//!
//! At ~120 ticks/s a buffer of `cap` frames covers roughly `cap / 120` seconds
//! of replay. Each snapshot stores the full `Vec<CellData>` (one byte per
//! cell plus padding) and the rigid block's pose. For a 1280x720 grid that's
//! ~920 KB per frame; 1500 frames is ~1.4 GB *uncompressed*. We keep only
//! `cap = 600` (~5 s) by default and rely on the OS page cache.

use super::block::Block;
use super::material::CellData;
use std::collections::VecDeque;

#[derive(Clone)]
pub struct Snapshot {
    pub cells: Vec<CellData>,
    pub block: Block,
}

pub struct History {
    pub snapshots: VecDeque<Snapshot>,
    pub cursor: usize,
    pub cap: usize,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self {
            snapshots: VecDeque::with_capacity(cap),
            cursor: 0,
            cap,
        }
    }

    /// Append a snapshot, evicting the oldest when at capacity. If the user
    /// was scrubbing into the past, anything after the cursor is forked off
    /// (replaced by the new state) — same semantics as a video editor.
    pub fn record(&mut self, cells: &[CellData], block: &Block) {
        if !self.snapshots.is_empty() && self.cursor + 1 < self.snapshots.len() {
            self.snapshots.truncate(self.cursor + 1);
        }
        if self.snapshots.len() >= self.cap {
            self.snapshots.pop_front();
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.snapshots.push_back(Snapshot {
            cells: cells.to_vec(),
            block: block.clone(),
        });
        self.cursor = self.snapshots.len() - 1;
    }

    pub fn scrub(&self, cursor: usize) -> Option<&Snapshot> {
        self.snapshots.get(cursor)
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::block::Block;
    use crate::sim::material::CellData;

    #[test]
    fn ring_buffer_evicts_oldest() {
        let mut h = History::new(3);
        let cells = vec![CellData::AIR; 4];
        let block = Block::new(0.0, 0.0, 1.0, 1.0);
        for _ in 0..5 {
            h.record(&cells, &block);
        }
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn scrubbing_then_recording_truncates_future() {
        use crate::sim::material::Cell;
        let mut h = History::new(10);
        let block = Block::new(0.0, 0.0, 1.0, 1.0);
        for i in 0..5 {
            let cell = CellData {
                kind: Cell::Sand,
                extra: i as u8,
            };
            h.record(std::slice::from_ref(&cell), &block);
        }
        h.cursor = 2;
        h.record(std::slice::from_ref(&CellData::AIR), &block);
        assert_eq!(h.len(), 4);
    }
}
