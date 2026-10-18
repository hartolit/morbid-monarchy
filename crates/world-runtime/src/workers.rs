//! Bounded workers for storage and terrain generation.
//!
//! The residency and checkpoint owners never touch the store from a frame: they hand jobs to
//! these threads and collect what they finished at the next frame. Reads and generation run on
//! a few loader threads; every write runs on one writer thread, in the order it was queued, so
//! a checkpoint's payloads, its body set and its publication reach the store in that order.
//! Both queues are bounded: a full queue refuses the job, and the owner tries again later.
//!
//! Every job carries the identity its result is matched with. A result for a request the owner
//! no longer wants (a cancelled load, a key that left and came back) is simply discarded by the
//! owner; nothing here decides what is current.

use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use world_model::{generation::WorldGenerator, resume::BodySet, spatial::ChunkKey};

use crate::{
    ids::{ChunkRevision, SessionEpoch, Tick},
    port::ResidentWindow,
    store::{
        CheckpointId, CheckpointManifest, ChunkPayload, ChunkRow, PhysicalConfig, Publication,
        ReclaimReport, StoreError, WorldStore,
    },
};

/// Thread and queue sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerConfig {
    pub loaders: u32,
    pub load_queue: u32,
    pub write_queue: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            loaders: 2,
            load_queue: 32,
            write_queue: 16,
        }
    }
}

/// Where a load's content comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    /// The key's published revision, or generated terrain when it has none.
    Published,
    /// An exact durable revision this session wrote.
    Revision(ChunkRevision),
    /// Generated terrain: the key has no saved revision.
    Generate,
}

/// What a load found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loaded {
    Stored(ChunkRevision),
    Generated,
}

/// A publication the writer performs, owned so it can cross to the thread.
#[derive(Debug, Clone)]
pub struct PublishJob {
    pub id: CheckpointId,
    pub tick: Option<Tick>,
    pub config: PhysicalConfig,
    pub window: ResidentWindow,
    pub epoch: SessionEpoch,
    pub heads: Vec<(ChunkKey, ChunkRevision)>,
    pub bodies: u32,
}

/// A batch of chunk revisions to make durable.
pub type WriteRows = Vec<(ChunkKey, ChunkRevision, Arc<ChunkPayload>)>;

enum LoadJob {
    Load {
        key: ChunkKey,
        request: u64,
        source: LoadSource,
        buffer: ChunkPayload,
    },
}

enum WriteJob {
    Chunks {
        batch: u64,
        rows: WriteRows,
    },
    Bodies {
        checkpoint: CheckpointId,
        bodies: Arc<BodySet>,
    },
    Publish(PublishJob),
    Reclaim {
        keep: Vec<(ChunkKey, ChunkRevision)>,
    },
}

/// A finished job.
pub enum Done {
    Loaded {
        key: ChunkKey,
        request: u64,
        result: Result<Loaded, StoreError>,
        buffer: ChunkPayload,
        /// How long the job waited in the queue and ran.
        age: Duration,
    },
    Written {
        batch: u64,
        rows: WriteRows,
        result: Result<(), StoreError>,
        age: Duration,
    },
    BodiesWritten {
        checkpoint: CheckpointId,
        result: Result<(), StoreError>,
    },
    Published {
        checkpoint: CheckpointId,
        result: Result<CheckpointManifest, StoreError>,
    },
    Reclaimed {
        result: Result<ReclaimReport, StoreError>,
    },
}

/// A job a full queue refused; the caller still owns it.
pub struct QueueFull<T>(pub T);

/// What closing the workers found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkersClosed {
    pub joined: bool,
    pub waited: Duration,
}

pub struct Workers {
    loads: Option<SyncSender<(LoadJob, Instant)>>,
    writes: Option<SyncSender<(WriteJob, Instant)>>,
    done: Receiver<Done>,
    threads: Vec<JoinHandle<()>>,
    /// Jobs handed over and not yet collected.
    loads_out: u32,
    writes_out: u32,
}

impl Workers {
    pub fn start(store: Arc<dyn WorldStore>, seed: u32, config: WorkerConfig) -> Self {
        let (done_tx, done) = mpsc::channel();
        let (loads, load_rx) = mpsc::sync_channel::<(LoadJob, Instant)>(config.load_queue as usize);
        let (writes, write_rx) =
            mpsc::sync_channel::<(WriteJob, Instant)>(config.write_queue as usize);
        let load_rx = Arc::new(Mutex::new(load_rx));
        let mut threads = Vec::new();
        for index in 0..config.loaders.max(1) {
            let receiver = Arc::clone(&load_rx);
            let store = Arc::clone(&store);
            let done = done_tx.clone();
            let thread = std::thread::Builder::new()
                .name(format!("world-load-{index}"))
                .spawn(move || load_thread(&*store, seed, &receiver, &done));
            if let Ok(thread) = thread {
                threads.push(thread);
            }
        }
        {
            let store = Arc::clone(&store);
            let done = done_tx;
            let thread = std::thread::Builder::new()
                .name("world-write".to_owned())
                .spawn(move || write_thread(&*store, &write_rx, &done));
            if let Ok(thread) = thread {
                threads.push(thread);
            }
        }
        Self {
            loads: Some(loads),
            writes: Some(writes),
            done,
            threads,
            loads_out: 0,
            writes_out: 0,
        }
    }

    pub fn loads_in_flight(&self) -> u32 {
        self.loads_out
    }

    pub fn writes_in_flight(&self) -> u32 {
        self.writes_out
    }

    pub fn load(
        &mut self,
        key: ChunkKey,
        request: u64,
        source: LoadSource,
        buffer: ChunkPayload,
    ) -> Result<(), QueueFull<ChunkPayload>> {
        let Some(loads) = &self.loads else {
            return Err(QueueFull(buffer));
        };
        let job = LoadJob::Load {
            key,
            request,
            source,
            buffer,
        };
        match loads.try_send((job, Instant::now())) {
            Ok(()) => {
                self.loads_out += 1;
                Ok(())
            }
            Err(TrySendError::Full((LoadJob::Load { buffer, .. }, _)))
            | Err(TrySendError::Disconnected((LoadJob::Load { buffer, .. }, _))) => {
                Err(QueueFull(buffer))
            }
        }
    }

    fn write(&mut self, job: WriteJob) -> Result<(), QueueFull<WriteJob>> {
        let Some(writes) = &self.writes else {
            return Err(QueueFull(job));
        };
        match writes.try_send((job, Instant::now())) {
            Ok(()) => {
                self.writes_out += 1;
                Ok(())
            }
            Err(TrySendError::Full((job, _))) | Err(TrySendError::Disconnected((job, _))) => {
                Err(QueueFull(job))
            }
        }
    }

    pub fn write_chunks(
        &mut self,
        batch: u64,
        rows: WriteRows,
    ) -> Result<(), QueueFull<WriteRows>> {
        self.write(WriteJob::Chunks { batch, rows })
            .map_err(|QueueFull(job)| match job {
                WriteJob::Chunks { rows, .. } => QueueFull(rows),
                _ => unreachable!("the refused job is the one sent"),
            })
    }

    pub fn write_bodies(&mut self, checkpoint: CheckpointId, bodies: Arc<BodySet>) -> bool {
        self.write(WriteJob::Bodies { checkpoint, bodies }).is_ok()
    }

    pub fn publish(&mut self, job: PublishJob) -> Result<(), QueueFull<PublishJob>> {
        self.write(WriteJob::Publish(job))
            .map_err(|QueueFull(job)| match job {
                WriteJob::Publish(job) => QueueFull(job),
                _ => unreachable!("the refused job is the one sent"),
            })
    }

    pub fn reclaim(&mut self, keep: Vec<(ChunkKey, ChunkRevision)>) -> bool {
        self.write(WriteJob::Reclaim { keep }).is_ok()
    }

    /// Collects finished jobs without blocking.
    pub fn collect(&mut self, into: &mut Vec<Done>) {
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Loaded { .. } => self.loads_out = self.loads_out.saturating_sub(1),
                _ => self.writes_out = self.writes_out.saturating_sub(1),
            }
            into.push(done);
        }
    }

    /// Waits up to `timeout` for every handed-over job to finish, collecting results. A
    /// maintenance wait for an orderly close; the frame path never calls it.
    pub fn finish(&mut self, timeout: Duration, into: &mut Vec<Done>) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.collect(into);
            if self.loads_out == 0 && self.writes_out == 0 {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            if let Ok(done) = self
                .done
                .recv_timeout((deadline - now).min(Duration::from_millis(50)))
            {
                match done {
                    Done::Loaded { .. } => self.loads_out = self.loads_out.saturating_sub(1),
                    _ => self.writes_out = self.writes_out.saturating_sub(1),
                }
                into.push(done);
            }
        }
    }

    /// Closes the queues and joins the threads within `timeout`. A thread still inside a store
    /// call when the time runs out is reported, not killed: it ends with the process.
    pub fn close(mut self, timeout: Duration, into: &mut Vec<Done>) -> WorkersClosed {
        let started = Instant::now();
        self.loads = None;
        self.writes = None;
        let deadline = started + timeout;
        let mut joined = true;
        for thread in std::mem::take(&mut self.threads) {
            while !thread.is_finished() && Instant::now() < deadline {
                self.collect(into);
                std::thread::sleep(Duration::from_millis(1));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                joined = false;
            }
        }
        self.collect(into);
        WorkersClosed {
            joined,
            waited: started.elapsed(),
        }
    }
}

fn load_thread(
    store: &dyn WorldStore,
    seed: u32,
    receiver: &Mutex<Receiver<(LoadJob, Instant)>>,
    done: &Sender<Done>,
) {
    let generator = WorldGenerator::new(seed);
    loop {
        let job = {
            let Ok(receiver) = receiver.lock() else {
                return;
            };
            receiver.recv()
        };
        let Ok((
            LoadJob::Load {
                key,
                request,
                source,
                mut buffer,
            },
            queued,
        )) = job
        else {
            return;
        };
        let generate = |buffer: &mut ChunkPayload| {
            let generated = generator.generate_chunk(key);
            buffer.cells_mut().copy_from_slice(&generated.cells);
            buffer.set_theme(generated.theme);
            Loaded::Generated
        };
        let result = match source {
            LoadSource::Generate => Ok(generate(&mut buffer)),
            LoadSource::Published => match store.read_chunk(key, None, &mut buffer) {
                Ok(Some(revision)) => Ok(Loaded::Stored(revision)),
                // Only a genuinely absent key is generated.
                Ok(None) => Ok(generate(&mut buffer)),
                Err(error) => Err(error),
            },
            LoadSource::Revision(revision) => {
                match store.read_chunk(key, Some(revision), &mut buffer) {
                    Ok(Some(found)) => Ok(Loaded::Stored(found)),
                    Ok(None) => Err(StoreError::Corrupt {
                        item: crate::store::StoredItem::Chunk(key, revision),
                        detail: "a revision this session made durable is missing",
                    }),
                    Err(error) => Err(error),
                }
            }
        };
        if done
            .send(Done::Loaded {
                key,
                request,
                result,
                buffer,
                age: queued.elapsed(),
            })
            .is_err()
        {
            return;
        }
    }
}

fn write_thread(
    store: &dyn WorldStore,
    receiver: &Receiver<(WriteJob, Instant)>,
    done: &Sender<Done>,
) {
    while let Ok((job, queued)) = receiver.recv() {
        let result = match job {
            WriteJob::Chunks { batch, rows } => {
                let borrowed: Vec<ChunkRow<'_>> = rows
                    .iter()
                    .map(|(key, revision, payload)| ChunkRow {
                        key: *key,
                        revision: *revision,
                        payload,
                    })
                    .collect();
                let result = store.write_chunks(&borrowed);
                drop(borrowed);
                Done::Written {
                    batch,
                    rows,
                    result,
                    age: queued.elapsed(),
                }
            }
            WriteJob::Bodies { checkpoint, bodies } => Done::BodiesWritten {
                checkpoint,
                result: store.write_bodies(checkpoint, &bodies),
            },
            WriteJob::Publish(job) => Done::Published {
                checkpoint: job.id,
                result: store.publish(&Publication {
                    id: job.id,
                    tick: job.tick,
                    config: job.config,
                    window: job.window,
                    epoch: job.epoch,
                    heads: &job.heads,
                    bodies: job.bodies,
                }),
            },
            WriteJob::Reclaim { keep } => Done::Reclaimed {
                result: store.reclaim(&keep),
            },
        };
        if done.send(result).is_err() {
            return;
        }
    }
}
