//! Pooled staging buffers for uploads and readbacks.
//!
//! A pool owns a fixed number of equally sized mappable buffers, created with the session. Each
//! buffer moves through one state machine:
//!
//! ```text
//! upload:   free (mapped) -> reserved (CPU writes) -> submitted -> remapping -> free
//! readback: free -> reserved -> submitted -> mapping -> ready (CPU reads) -> free
//! ```
//!
//! Every reservation advances the buffer's generation. A map callback records the generation it
//! was registered for in a fixed mailbox entry per buffer; a delivery whose generation no longer
//! matches is stale and ignored, so a late callback can never complete a later reservation.
//! Callbacks only write the mailbox; the owner reads it on its own thread and never holds the
//! mailbox lock while it polls the device, which is where callbacks run.
//!
//! An exhausted pool refuses the reservation; it never overwrites a buffer in use.

use std::sync::{Arc, Mutex};

use crate::context::{AllocationError, GpuContext};

/// A reservation of one staging buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StagingRef {
    pub index: u32,
    pub generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Readback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Reserved,
    Submitted {
        serial: u64,
    },
    /// A map request is outstanding (upload: remapping for the next write).
    Mapping,
    /// Readback data is mapped and waiting for the consumer.
    Ready,
    /// A map failed; the buffer is unusable for the rest of the session.
    Failed,
}

struct Buffer {
    buffer: wgpu::Buffer,
    generation: u32,
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Delivery {
    generation: u32,
    ok: bool,
}

/// A completed map, as the owner sees it after [`StagingPool::collect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// Readback data is mapped and ready.
    Ready(StagingRef),
    /// An upload buffer is mapped and free again.
    Remapped(StagingRef),
    Failed(StagingRef),
}

/// How collected deliveries are observed; fault injection holds or reorders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliveryOrder {
    /// Leave deliveries in the mailbox for a later collection.
    pub hold: bool,
    /// Observe deliveries in reverse buffer order.
    pub reverse: bool,
}

pub struct StagingPool {
    label: &'static str,
    direction: Direction,
    size: u64,
    buffers: Vec<Buffer>,
    mailbox: Arc<Mutex<Vec<Option<Delivery>>>>,
    high_water: u32,
    stale: u64,
}

impl StagingPool {
    pub fn new(
        context: &mut GpuContext,
        label: &'static str,
        direction: Direction,
        count: u32,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> Result<Self, AllocationError> {
        let mut buffers = Vec::with_capacity(count as usize);
        for _ in 0..count {
            // Upload buffers start mapped, so the first write needs no map round trip.
            let mapped = direction == Direction::Upload;
            let buffer = context.create_buffer(label, size, usage, mapped);
            match buffer {
                Ok(buffer) => buffers.push(Buffer {
                    buffer,
                    generation: 0,
                    state: State::Free,
                }),
                Err(error) => {
                    for created in buffers {
                        context.release_buffer(created.buffer);
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            label,
            direction,
            size,
            buffers,
            mailbox: Arc::new(Mutex::new(vec![None; count as usize])),
            high_water: 0,
            stale: 0,
        })
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn capacity(&self) -> u32 {
        self.buffers.len() as u32
    }

    pub fn in_use(&self) -> u32 {
        self.buffers
            .iter()
            .filter(|buffer| buffer.state != State::Free)
            .count() as u32
    }

    pub fn high_water(&self) -> u32 {
        self.high_water
    }

    /// Deliveries that arrived for a reservation that no longer existed.
    pub fn stale_deliveries(&self) -> u64 {
        self.stale
    }

    pub fn has_free(&self) -> bool {
        self.buffers
            .iter()
            .any(|buffer| buffer.state == State::Free)
    }

    /// Reserves a free buffer, or returns `None` when every buffer is in use.
    pub fn reserve(&mut self) -> Option<StagingRef> {
        let index = self
            .buffers
            .iter()
            .position(|buffer| buffer.state == State::Free)?;
        let buffer = &mut self.buffers[index];
        buffer.generation = buffer.generation.wrapping_add(1);
        buffer.state = State::Reserved;
        let in_use = self.in_use();
        self.high_water = self.high_water.max(in_use);
        Some(StagingRef {
            index: index as u32,
            generation: self.buffers[index].generation,
        })
    }

    fn current(&self, reservation: StagingRef) -> Option<&Buffer> {
        self.buffers
            .get(reservation.index as usize)
            .filter(|buffer| buffer.generation == reservation.generation)
    }

    /// The buffer of a live reservation, for encoding copies.
    pub fn buffer(&self, reservation: StagingRef) -> Option<&wgpu::Buffer> {
        self.current(reservation).map(|buffer| &buffer.buffer)
    }

    /// Writes `bytes` at `offset` into a reserved, mapped upload buffer.
    pub fn write(&self, reservation: StagingRef, offset: u64, bytes: &[u8]) -> bool {
        let Some(buffer) = self.current(reservation) else {
            return false;
        };
        if self.direction != Direction::Upload
            || buffer.state != State::Reserved
            || bytes.is_empty()
        {
            return bytes.is_empty();
        }
        let end = offset + bytes.len() as u64;
        let mut view = buffer.buffer.get_mapped_range_mut(offset..end);
        view.copy_from_slice(bytes);
        true
    }

    /// Unmaps a written upload buffer before a submission copies from it.
    pub fn unmap_for_copy(&self, reservation: StagingRef) {
        if let Some(buffer) = self.current(reservation)
            && self.direction == Direction::Upload
            && buffer.state == State::Reserved
        {
            buffer.buffer.unmap();
        }
    }

    /// Records that `serial` uses the reservation. A readback buffer is mapped as soon as that
    /// submission completes; the callback only writes the mailbox.
    pub fn submitted(&mut self, reservation: StagingRef, serial: u64) {
        let mailbox = Arc::clone(&self.mailbox);
        let direction = self.direction;
        let Some(buffer) = self
            .buffers
            .get_mut(reservation.index as usize)
            .filter(|buffer| buffer.generation == reservation.generation)
        else {
            return;
        };
        buffer.state = State::Submitted { serial };
        if direction == Direction::Readback {
            buffer.state = State::Mapping;
            let index = reservation.index as usize;
            let generation = reservation.generation;
            buffer
                .buffer
                .map_async(wgpu::MapMode::Read, .., move |result| {
                    if let Ok(mut deliveries) = mailbox.lock() {
                        deliveries[index] = Some(Delivery {
                            generation,
                            ok: result.is_ok(),
                        });
                    }
                });
        }
    }

    /// Requests remapping of upload buffers whose submissions completed.
    pub fn completed(&mut self, completed_serial: u64) {
        if self.direction != Direction::Upload {
            return;
        }
        for (index, buffer) in self.buffers.iter_mut().enumerate() {
            let State::Submitted { serial } = buffer.state else {
                continue;
            };
            if serial > completed_serial {
                continue;
            }
            buffer.state = State::Mapping;
            let mailbox = Arc::clone(&self.mailbox);
            let generation = buffer.generation;
            buffer
                .buffer
                .map_async(wgpu::MapMode::Write, .., move |result| {
                    if let Ok(mut deliveries) = mailbox.lock() {
                        deliveries[index] = Some(Delivery {
                            generation,
                            ok: result.is_ok(),
                        });
                    }
                });
        }
    }

    /// Moves delivered maps into their next state and appends what happened to `into`.
    pub fn collect(&mut self, order: DeliveryOrder, into: &mut Vec<Completion>) {
        if order.hold {
            return;
        }
        let Ok(mut deliveries) = self.mailbox.lock() else {
            return;
        };
        let count = deliveries.len();
        for step in 0..count {
            let index = if order.reverse {
                count - 1 - step
            } else {
                step
            };
            let Some(delivery) = deliveries[index].take() else {
                continue;
            };
            let buffer = &mut self.buffers[index];
            if buffer.generation != delivery.generation || buffer.state != State::Mapping {
                self.stale += 1;
                continue;
            }
            let reservation = StagingRef {
                index: index as u32,
                generation: delivery.generation,
            };
            if !delivery.ok {
                buffer.state = State::Failed;
                into.push(Completion::Failed(reservation));
                continue;
            }
            match self.direction {
                Direction::Upload => {
                    buffer.state = State::Free;
                    into.push(Completion::Remapped(reservation));
                }
                Direction::Readback => {
                    buffer.state = State::Ready;
                    into.push(Completion::Ready(reservation));
                }
            }
        }
    }

    /// Whether a readback reservation holds mapped data.
    pub fn is_ready(&self, reservation: StagingRef) -> bool {
        self.current(reservation)
            .is_some_and(|buffer| buffer.state == State::Ready)
    }

    /// Reads a ready readback buffer's bytes in `range`.
    pub fn read<T>(
        &self,
        reservation: StagingRef,
        range: std::ops::Range<u64>,
        read: impl FnOnce(&[u8]) -> T,
    ) -> Option<T> {
        let buffer = self.current(reservation)?;
        if buffer.state != State::Ready || range.is_empty() {
            return None;
        }
        let view = buffer.buffer.get_mapped_range(range);
        Some(read(&view))
    }

    /// Ends a reservation: unmaps a readback buffer, or returns an unsubmitted upload buffer
    /// (still mapped) to the pool.
    pub fn release(&mut self, reservation: StagingRef) {
        let direction = self.direction;
        let Some(buffer) = self
            .buffers
            .get_mut(reservation.index as usize)
            .filter(|buffer| buffer.generation == reservation.generation)
        else {
            return;
        };
        match (direction, buffer.state) {
            (Direction::Readback, State::Ready) => {
                buffer.buffer.unmap();
                buffer.state = State::Free;
            }
            (_, State::Reserved) => buffer.state = State::Free,
            _ => {}
        }
    }

    /// Releases every buffer's bytes; the pool is unusable afterwards.
    pub fn destroy(self, context: &mut GpuContext) {
        for buffer in self.buffers {
            context.release_buffer(buffer.buffer);
        }
    }
}
