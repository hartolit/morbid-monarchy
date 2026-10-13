//! Identities that cross the CPU–GPU boundary: world, session, command, tick and revision.
//!
//! Each answers a different question, so each has its own type. A session ends when its GPU
//! device does; the next session of the same world has a later epoch, and anything carrying an
//! earlier epoch (a slot reference, a ticket, a late callback) is stale by construction.

use std::fmt;

/// Identity of one world: a saved world, or a scratch world a test or benchmark generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldId(u64);

impl WorldId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// One live session of a world on one GPU device. Device loss or recreation starts a new epoch.
///
/// The epoch is a `u32` so a shader can echo it in one word. A world that has been recreated
/// four billion times refuses the next session instead of wrapping (see [`SessionEpoch::next`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionEpoch(u32);

impl SessionEpoch {
    pub const FIRST: Self = Self(1);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// The epoch of the session that replaces this one, or `None` when the epochs are exhausted.
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// A world together with the session epoch an object belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId {
    pub world: WorldId,
    pub epoch: SessionEpoch,
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world {} epoch {}", self.world.0, self.epoch.0)
    }
}

/// A command's identity within one session, chosen by whoever issues the command.
///
/// Retrying with the same identity and the same command never applies it twice; reusing an
/// identity for a different command is refused. Identities below the acknowledged floor have
/// expired (see `receipts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(u64);

impl CommandId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    /// The identity as two 32-bit words, low word first, for a GPU record.
    pub const fn to_words(self) -> [u32; 2] {
        [self.0 as u32, (self.0 >> 32) as u32]
    }

    pub const fn from_words(words: [u32; 2]) -> Self {
        Self(words[0] as u64 | ((words[1] as u64) << 32))
    }
}

/// One logical step of the physical world.
///
/// Ticks are numbered from 0 and only advance when the runtime submits one. A `u64` does not
/// roll over in practice (at 240 ticks per second it lasts 2.4 billion years); reaching the end
/// is still a refusal, never a wrap. Shaders receive it as two words, low word first, and need
/// no 64-bit arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Tick(u64);

impl Tick {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Ticks from `earlier` to `self`; zero when `earlier` is not earlier.
    pub const fn since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    pub const fn to_words(self) -> [u32; 2] {
        [self.0 as u32, (self.0 >> 32) as u32]
    }

    pub const fn from_words(words: [u32; 2]) -> Self {
        Self(words[0] as u64 | ((words[1] as u64) << 32))
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tick {}", self.0)
    }
}

/// A chunk's content version. It advances whenever the chunk's cells change; a saved or
/// captured payload is identified by the revision it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ChunkRevision(u64);

impl ChunkRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn to_words(self) -> [u32; 2] {
        [self.0 as u32, (self.0 >> 32) as u32]
    }

    pub const fn from_words(words: [u32; 2]) -> Self {
        Self(words[0] as u64 | ((words[1] as u64) << 32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_identities_split_into_low_then_high_words() {
        let id = CommandId::new(0x0123_4567_89AB_CDEF);
        assert_eq!(id.to_words(), [0x89AB_CDEF, 0x0123_4567]);
        assert_eq!(CommandId::from_words(id.to_words()), id);
        let tick = Tick::new(u64::from(u32::MAX) + 2);
        assert_eq!(tick.to_words(), [1, 1]);
        assert_eq!(Tick::from_words([1, 1]), tick);
        let revision = ChunkRevision::new(7 << 32 | 9);
        assert_eq!(ChunkRevision::from_words(revision.to_words()), revision);
    }

    #[test]
    fn counters_refuse_to_wrap() {
        assert_eq!(Tick::new(u64::MAX).next(), None);
        assert_eq!(Tick::ZERO.next(), Some(Tick::new(1)));
        assert_eq!(SessionEpoch::new(u32::MAX).next(), None);
        assert_eq!(SessionEpoch::FIRST.next(), Some(SessionEpoch::new(2)));
        assert_eq!(Tick::new(3).since(Tick::new(5)), 0);
        assert_eq!(Tick::new(5).since(Tick::new(3)), 2);
    }
}
