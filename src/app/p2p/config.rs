use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pServiceConfig {
    pub keypair_file: PathBuf,
    pub file_search_topic: Option<String>,
}

impl P2pServiceConfig {
    pub fn builder() -> P2pServiceConfigBuilder {
        P2pServiceConfigBuilder::new()
    }
}

#[derive(Debug, Clone)]
pub struct P2pServiceConfigBuilder {
    config: P2pServiceConfig,
}

impl P2pServiceConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: P2pServiceConfig::default(),
        }
    }

    pub fn with_keypair_file<P: AsRef<Path>>(mut self, keypair_file: P) -> Self {
        self.config.keypair_file = keypair_file.as_ref().into();
        self
    }

    pub fn with_file_search_topic(mut self, topic: String) -> Self {
        self.config.file_search_topic = Some(topic);
        self
    }

    pub fn build(self) -> P2pServiceConfig {
        self.config.clone()
    }
}
