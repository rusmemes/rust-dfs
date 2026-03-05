use crate::app::file_processing::processing::FileMetadata;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublishedFileRecord {
    pub key: PublishedFileKey,
    pub original_file_name: String,
    pub chunks_dir: PathBuf,
    pub public: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq, Hash)]
pub struct PublishedFileKey(pub [u8; 8]);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PendingDownloadRecord {
    pub key: PublishedFileKey,
    pub original_file_name: String,
    pub chunks_dir: PathBuf,
    pub downloaded_chunks: HashSet<usize>,
}

impl PendingDownloadRecord {
    pub fn new(key: PublishedFileKey, original_file_name: String, chunks_dir: PathBuf) -> Self {
        Self {
            key,
            original_file_name,
            chunks_dir,
            downloaded_chunks: HashSet::new(),
        }
    }
}

pub trait Persistable:
    TryInto<(PublishedFileKey, Vec<u8>), Error = serde_cbor::Error> + Send + 'static
{
}

impl<T> Persistable for T where
    T: TryInto<(PublishedFileKey, Vec<u8>), Error = serde_cbor::Error> + Send + 'static
{
}

pub trait Iterable: TryFrom<Vec<u8>, Error = serde_cbor::Error> + Send + 'static {}
impl<T> Iterable for T where T: TryFrom<Vec<u8>, Error = serde_cbor::Error> + Send + 'static {}

impl TryInto<(PublishedFileKey, Vec<u8>)> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<(PublishedFileKey, Vec<u8>), Self::Error> {
        let key = self.key.clone();
        TryInto::try_into(self).map(|bytes| (key, bytes))
    }
}

impl TryInto<(PublishedFileKey, Vec<u8>)> for PendingDownloadRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<(PublishedFileKey, Vec<u8>), Self::Error> {
        let key = self.key.clone();
        TryInto::try_into(self).map(|bytes| (key, bytes))
    }
}

impl From<PublishedFileKey> for u64 {
    fn from(key: PublishedFileKey) -> u64 {
        u64::from_be_bytes(key.0)
    }
}

impl From<[u8; 8]> for PublishedFileKey {
    fn from(key: [u8; 8]) -> PublishedFileKey {
        PublishedFileKey(key)
    }
}

impl From<PublishedFileKey> for [u8; 8] {
    fn from(value: PublishedFileKey) -> Self {
        value.0
    }
}

impl From<u64> for PublishedFileKey {
    fn from(key: u64) -> PublishedFileKey {
        PublishedFileKey(key.to_be_bytes())
    }
}

impl From<FileMetadata> for PublishedFileRecord {
    fn from(result: FileMetadata) -> Self {
        Self {
            key: result.key().into(),
            original_file_name: result.original_file_name,
            chunks_dir: result.chunks_dir,
            public: result.public,
        }
    }
}

impl TryInto<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

impl TryFrom<Vec<u8>> for PublishedFileRecord {
    type Error = serde_cbor::Error;
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(&bytes)
    }
}

impl TryInto<Vec<u8>> for PendingDownloadRecord {
    type Error = serde_cbor::Error;

    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_cbor::to_vec(&self)
    }
}

impl TryFrom<Vec<u8>> for PendingDownloadRecord {
    type Error = serde_cbor::Error;
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        serde_cbor::from_slice(&bytes)
    }
}
