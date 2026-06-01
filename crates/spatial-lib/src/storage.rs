use crate::math::ChunkKey;

/// Defines the physical boundary for chunk persistence.
/// Enforces raw byte slice (`&[u8]`) I/O to prohibit serialization logic from bleeding into the storage backend.
pub trait ChunkStorage: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetches a raw byte payload corresponding to the given coordinate.
    fn read_chunk(&self, key: ChunkKey) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Commits a single raw byte payload to the physical media.
    fn write_chunk(&self, key: ChunkKey, data: &[u8]) -> Result<(), Self::Error>;

    /// Commits multiple chunks in a single atomic transaction.
    fn write_batch(&self, chunks: &[(ChunkKey, &[u8])]) -> Result<(), Self::Error>;
}

#[cfg(feature = "redb-storage")]
pub mod redb_backend {
    use super::ChunkStorage;
    use crate::math::ChunkKey;
    use redb::{Database, ReadableDatabase, TableDefinition};
    use std::sync::Arc;

    const CHUNKS_TABLE: TableDefinition<[i32; 3], &[u8]> = TableDefinition::new("spatial_chunks");

    /// Represents catastrophic failures at the local storage boundary.
    #[derive(Debug)]
    pub enum RedbStorageError {
        DatabaseError(redb::Error),
        TransactionError(redb::TransactionError),
        TableError(redb::TableError),
        StorageError(redb::StorageError),
        CommitError(redb::CommitError),
    }

    impl std::fmt::Display for RedbStorageError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::DatabaseError(e) => write!(f, "Database error: {}", e),
                Self::TransactionError(e) => write!(f, "Transaction error: {}", e),
                Self::TableError(e) => write!(f, "Table error: {}", e),
                Self::StorageError(e) => write!(f, "Storage error: {}", e),
                Self::CommitError(e) => write!(f, "Commit error: {}", e),
            }
        }
    }

    impl std::error::Error for RedbStorageError {}

    impl From<redb::Error> for RedbStorageError {
        fn from(e: redb::Error) -> Self {
            Self::DatabaseError(e)
        }
    }
    impl From<redb::TransactionError> for RedbStorageError {
        fn from(e: redb::TransactionError) -> Self {
            Self::TransactionError(e)
        }
    }
    impl From<redb::TableError> for RedbStorageError {
        fn from(e: redb::TableError) -> Self {
            Self::TableError(e)
        }
    }
    impl From<redb::StorageError> for RedbStorageError {
        fn from(e: redb::StorageError) -> Self {
            Self::StorageError(e)
        }
    }
    impl From<redb::CommitError> for RedbStorageError {
        fn from(e: redb::CommitError) -> Self {
            Self::CommitError(e)
        }
    }

    /// A synchronous, thread-safe implementation of `ChunkStorage` utilizing the `redb` B-Tree backend.
    pub struct RedbChunkStorage {
        db: Arc<Database>,
    }

    impl RedbChunkStorage {
        /// Bootstraps the database connection and ensures the physical table schema exists.
        pub fn new(db: Arc<Database>) -> Result<Self, RedbStorageError> {
            let write_txn = db.begin_write()?;
            {
                let _table = write_txn.open_table(CHUNKS_TABLE)?;
            }
            write_txn.commit()?;
            Ok(Self { db })
        }
    }

    impl ChunkStorage for RedbChunkStorage {
        type Error = RedbStorageError;

        fn read_chunk(&self, key: ChunkKey) -> Result<Option<Vec<u8>>, Self::Error> {
            let read_txn = self.db.begin_read()?;
            let table = read_txn.open_table(CHUNKS_TABLE)?;

            if let Some(access) = table.get([key.key.x, key.key.y, key.key.z])? {
                Ok(Some(access.value().to_vec()))
            } else {
                Ok(None)
            }
        }

        fn write_chunk(&self, key: ChunkKey, data: &[u8]) -> Result<(), Self::Error> {
            let write_txn = self.db.begin_write()?;
            {
                let mut table = write_txn.open_table(CHUNKS_TABLE)?;
                table.insert([key.key.x, key.key.y, key.key.z], data)?;
            }
            write_txn.commit()?;
            Ok(())
        }

        fn write_batch(&self, chunks: &[(ChunkKey, &[u8])]) -> Result<(), Self::Error> {
            if chunks.is_empty() {
                return Ok(());
            }

            let write_txn = self.db.begin_write()?;
            {
                let mut table = write_txn.open_table(CHUNKS_TABLE)?;
                for (key, data) in chunks {
                    table.insert([key.key.x, key.key.y, key.key.z], *data)?;
                }
            }
            write_txn.commit()?;
            Ok(())
        }
    }
}
