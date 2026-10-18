//! Fault injection at the storage boundary, for tests of what the owners do when storage fails.
//!
//! Three kinds of fault, which tests must keep apart:
//! - **I/O errors** come from [`FaultBackend`], a redb `StorageBackend` around the real file:
//!   the next writes or syncs of the database file fail, inside redb's own commit, exactly as a
//!   full or failing disk would. redb then refuses further transactions until the store reopens
//!   the file, which is what a real failure costs.
//! - **Returned errors** at a [`Barrier`] point of the store's own code: the operation stops
//!   with an I/O error before touching the file. This is a simulation, reported as one.
//! - **Process interruption**: at a barrier point, or at the n-th file write of the next commit,
//!   the process aborts, leaving whatever the file holds. Only a reopen can tell what survived.
//!
//! A barrier can also hold an operation until the test releases it, so a save can be delayed
//! while the rest of the program runs on. A store without faults uses a controller nobody arms.

use std::{
    fs::File,
    io,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use redb::{StorageBackend, backends::FileBackend};

/// A point in the store's operations where a fault can be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Barrier {
    /// Chunk rows are staged in a transaction that has not committed.
    BeforeChunkCommit,
    /// The chunk transaction committed.
    AfterChunkCommit,
    /// A body set is staged and not committed.
    BeforeBodiesCommit,
    /// A publication's heads, manifest and latest pointer are staged, not committed.
    BeforePublishCommit,
    /// The publication committed; superseded rows are not yet reclaimed.
    AfterPublishCommit,
}

/// What happens when an operation reaches an armed barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Wait until [`StoreFaults::release`] (or the time limit, which fails the operation).
    Hold { limit: Duration },
    /// Fail the operation with an I/O error, changing nothing.
    Fail,
    /// End the process at once, as a crash would.
    Abort,
}

#[derive(Debug, Default)]
struct State {
    armed: Vec<(Barrier, Action, u32)>,
    holding: Option<Barrier>,
    released: u64,
    reached: Vec<Barrier>,
    fail_writes: u64,
    fail_syncs: u64,
    abort_after_writes: Option<u64>,
    writes: u64,
    syncs: u64,
    injected: u64,
}

/// The controller tests hold. Cloning shares it; every file the store opens uses it.
#[derive(Debug, Clone, Default)]
pub struct StoreFaults {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl StoreFaults {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A panicking holder cannot leave the counters in a state worse than stale, and fault
        // injection must keep working for the test that observes the panic.
        self.state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Arms `action` at `barrier` for the next `times` operations that reach it.
    pub fn arm(&self, barrier: Barrier, action: Action, times: u32) {
        self.lock().armed.push((barrier, action, times));
    }

    /// Removes every action armed at `barrier`.
    pub fn disarm(&self, barrier: Barrier) {
        self.lock().armed.retain(|(armed, ..)| *armed != barrier);
    }

    /// Lets a held operation continue.
    pub fn release(&self) {
        let mut state = self.lock();
        state.released += 1;
        state.holding = None;
        self.state.1.notify_all();
    }

    /// The barrier an operation is held at, if any.
    pub fn holding(&self) -> Option<Barrier> {
        self.lock().holding
    }

    /// Barriers reached so far, in order.
    pub fn reached(&self) -> Vec<Barrier> {
        self.lock().reached.clone()
    }

    /// Fails the next `count` writes to the database file with an I/O error.
    pub fn fail_writes(&self, count: u64) {
        self.lock().fail_writes = count;
    }

    /// Fails the next `count` syncs of the database file.
    pub fn fail_syncs(&self, count: u64) {
        self.lock().fail_syncs = count;
    }

    /// Aborts the process at the `count`-th file write from now: a crash in the middle of a
    /// commit.
    pub fn abort_after_writes(&self, count: u64) {
        self.lock().abort_after_writes = Some(count);
    }

    /// File writes and syncs so far, and faults injected.
    pub fn io_counts(&self) -> (u64, u64, u64) {
        let state = self.lock();
        (state.writes, state.syncs, state.injected)
    }

    /// Called by the store at `barrier`. `Err` means the operation must stop and change nothing.
    pub(crate) fn reach(&self, barrier: Barrier) -> Result<(), String> {
        let mut state = self.lock();
        state.reached.push(barrier);
        let Some(position) = state
            .armed
            .iter()
            .position(|(armed, _, times)| *armed == barrier && *times > 0)
        else {
            return Ok(());
        };
        let action = state.armed[position].1;
        state.armed[position].2 -= 1;
        state.injected += 1;
        match action {
            Action::Fail => Err(format!("injected failure at {barrier:?}")),
            Action::Abort => std::process::abort(),
            Action::Hold { limit } => {
                state.holding = Some(barrier);
                let released = state.released;
                let (state, timeout) = self
                    .state
                    .1
                    .wait_timeout_while(state, limit, |state| state.released == released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(state);
                if timeout.timed_out() {
                    let mut state = self.lock();
                    state.holding = None;
                    return Err(format!("held at {barrier:?} past {limit:?}"));
                }
                Ok(())
            }
        }
    }

    fn on_write(&self) -> io::Result<()> {
        let mut state = self.lock();
        state.writes += 1;
        if let Some(remaining) = state.abort_after_writes.as_mut() {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                std::process::abort();
            }
        }
        if state.fail_writes > 0 {
            state.fail_writes -= 1;
            state.injected += 1;
            return Err(io::Error::other("injected write failure"));
        }
        Ok(())
    }

    fn on_sync(&self) -> io::Result<()> {
        let mut state = self.lock();
        state.syncs += 1;
        if state.fail_syncs > 0 {
            state.fail_syncs -= 1;
            state.injected += 1;
            return Err(io::Error::other("injected sync failure"));
        }
        Ok(())
    }
}

/// redb's file backend with injected I/O faults. Reads are never failed: a read error is the
/// owners' corrupt-or-unavailable case, which tests produce with real damaged rows instead.
#[derive(Debug)]
pub struct FaultBackend {
    file: FileBackend,
    faults: StoreFaults,
}

impl FaultBackend {
    pub fn new(file: File, faults: StoreFaults) -> Result<Self, redb::DatabaseError> {
        Ok(Self {
            file: FileBackend::new(file)?,
            faults,
        })
    }
}

impl StorageBackend for FaultBackend {
    fn len(&self) -> Result<u64, io::Error> {
        self.file.len()
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), io::Error> {
        self.file.read(offset, out)
    }

    fn set_len(&self, len: u64) -> Result<(), io::Error> {
        self.file.set_len(len)
    }

    fn sync_data(&self) -> Result<(), io::Error> {
        self.faults.on_sync()?;
        self.file.sync_data()
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
        self.faults.on_write()?;
        self.file.write(offset, data)
    }

    fn close(&self) -> Result<(), io::Error> {
        self.file.close()
    }
}
