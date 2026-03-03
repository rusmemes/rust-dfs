use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::domain::{
    PendingDownloadRecord, PublishedFileKey, PublishedFileRecord,
};
use crate::app::file_store::errors::FileStoreError;
use crate::app::file_store::FileStore;
use crate::app::p2p::domain::{
    FileRequest, FileResponse, P2pCommand, P2pNetworkBehaviour, P2pNetworkBehaviourEvent,
    PublishedFile,
};
use crate::app::p2p::errors::P2pNetworkError;
use crate::app::utils::METADATA_FILE_NAME;
use libp2p::futures::StreamExt;
use libp2p::kad::{
    GetProvidersOk, GetProvidersResult, QueryId, QueryResult, Quorum, Record, RecordKey,
};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{OutboundRequestId, ResponseChannel};
use libp2p::swarm::SwarmEvent;
use libp2p::{gossipsub, identify, kad, mdns, relay, request_response, PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};
use tokio::time::sleep;

const LOG_TARGET: &str = "app::p2p::events";

pub struct MetadataProvidersRequestData {
    found_providers: HashSet<PeerId>,
    request: FileRequest,
    result: oneshot::Sender<Option<FileProcessingResult>>,
}

// possible a memory leak for maps caching queries and requests data
pub struct EventService<S: FileStore> {
    pub swarm: Swarm<P2pNetworkBehaviour>,
    store: S,
    metadata_providers_requests: HashMap<QueryId, MetadataProvidersRequestData>,
    metadata_download_requests:
        HashMap<OutboundRequestId, oneshot::Sender<Option<FileProcessingResult>>>,
    active_downloads: Arc<RwLock<HashSet<PublishedFileKey>>>,
}

impl<S: FileStore> EventService<S> {
    pub fn new(swarm: Swarm<P2pNetworkBehaviour>, store: S) -> Self {
        Self {
            swarm,
            store,
            metadata_providers_requests: HashMap::new(),
            metadata_download_requests: HashMap::new(),
            active_downloads: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    async fn start_download(pending_download_record: &PendingDownloadRecord) -> anyhow::Result<()> {
        todo!()
    }

    pub async fn work_on_pending_downloads(&mut self) {
        let mut stream = self.store.stream_pending_downloads();
        while let Some(result) = stream.next().await {
            match result {
                Ok(pending_download_record) => {
                    let active_downloads = self.active_downloads.clone();

                    if active_downloads
                        .write()
                        .await
                        .insert(pending_download_record.key.clone())
                    {
                        tokio::task::spawn(async move {
                            if let Err(error) = Self::start_download(&pending_download_record).await
                            {
                                error!("Failed to start download: {}", error);
                                active_downloads
                                    .write()
                                    .await
                                    .remove(&pending_download_record.key);
                            }
                        });
                    }
                }
                Err(error) => {
                    error!(target: LOG_TARGET, "Error reading from stream: {}", error);
                }
            }
        }
    }

    pub async fn handle_command(&mut self, command: P2pCommand) {
        match command {
            P2pCommand::RequestMetadata { request, result } => {
                let key: PublishedFileKey = request.file_id.into();
                let key = RecordKey::new(&key.0);
                let query_id = self.swarm.behaviour_mut().kademlia.get_providers(key);
                self.metadata_providers_requests.insert(
                    query_id,
                    MetadataProvidersRequestData {
                        found_providers: HashSet::new(),
                        request,
                        result,
                    },
                );
            }
        }
    }

    async fn provide(
        &mut self,
        file_processing_result: &FileProcessingResult,
    ) -> Result<(), P2pNetworkError> {
        let raw_key = file_processing_result.key();

        let value = serde_cbor::to_vec(&PublishedFile {
            total_chunks: file_processing_result.total_chunks,
            merkle_root: file_processing_result.merkle_root,
        })?;

        let record = Record::new(raw_key.to_vec(), value);
        let record_key = record.key.clone();

        self.swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, Quorum::Majority)?;

        self.swarm
            .behaviour_mut()
            .kademlia
            .start_providing(record_key)?;

        info!(target: LOG_TARGET, "Provided record key: {:?}", u64::from(PublishedFileKey(raw_key)));

        Ok(())
    }

    pub async fn provide_all_published_files(&mut self) -> Result<(), P2pNetworkError> {
        let mut receiver_stream = self.store.stream_published_files();

        while let Some(result) = receiver_stream.next().await {
            let published_file = result?;
            let metadata_buf = published_file.target_dir.join(METADATA_FILE_NAME);
            match tokio::fs::read(metadata_buf).await {
                Ok(data) => self.provide(&data.try_into()?).await?,
                Err(error) => error!(target: LOG_TARGET, "Error reading metadata file: {}", error),
            };
        }

        Ok(())
    }

    pub async fn file_publish(&mut self, result: Result<FileProcessingResult, FileStoreError>) {
        match result {
            Ok(file_processing_result) => {
                while let Err(error) = self.provide(&file_processing_result).await {
                    error!(target: LOG_TARGET, "Error providing file: {}", error);
                    sleep(Duration::from_secs(1)).await
                }

                let raw_key = file_processing_result.key();
                self.add_published_file(file_processing_result).await;
                self.delete_file_processing_result(raw_key).await;
                info!("Successfully published a file");
            }
            Err(error) => {
                error!(target: LOG_TARGET, "File store error: {:?}", error);
            }
        }
    }

    async fn add_published_file(&mut self, file_processing_result: FileProcessingResult) {
        let published_file_record: PublishedFileRecord = file_processing_result.into();
        while let Err(error) = self
            .store
            .add_published_file(published_file_record.clone())
            .await
        {
            error!("Failed to add published file: {}", error);
            sleep(Duration::from_secs(1)).await
        }
    }

    async fn delete_file_processing_result(&mut self, file_processing_result_key: [u8; 8]) {
        loop {
            match self
                .store
                .delete_file_processing_result(file_processing_result_key.clone())
                .await
            {
                Err(error) => {
                    error!(target: LOG_TARGET, "Error deleting file split record: {}", error);
                    sleep(Duration::from_secs(1)).await
                }
                _ => break,
            }
        }
    }

    pub async fn handle_swarm_event(&mut self, event: SwarmEvent<P2pNetworkBehaviourEvent>) {
        use P2pNetworkBehaviourEvent::*;
        use SwarmEvent::*;
        match event {
            Behaviour(event) => match event {
                Identify(event) => self.identify(event),
                Mdns(event) => self.mdns(event),
                Kademlia(event) => self.kademlia(event),
                Gossipsub(event) => self.gossipsub(event),
                RelayServer(event) => log_debug(&event),
                RelayClient(event) => log_debug(&event),
                Dcutr(event) => log_debug(&event),
                FileDownload(event) => self.file_download(event),
                MetadataDownload(event) => self.metadata_download(event).await,
                _ => log_debug(&event),
            },
            NewListenAddr {
                listener_id: _listener_id,
                address,
            } => info!(target: LOG_TARGET, "Listening on {:?}", address),
            _ => log_debug(&event),
        }
    }

    fn kademlia(&mut self, event: kad::Event) {
        use kad::Event::*;
        match event {
            OutboundQueryProgressed {
                id,
                result,
                stats: _stats,
                step: _step,
            } => {
                self.handle_metadata_providers_query_progressed(id, result);
            }
            _ => log_debug(&event),
        }
    }

    fn handle_metadata_providers_query_progressed(&mut self, id: QueryId, result: QueryResult) {
        if let QueryResult::GetProviders(result) = result {
            self.handle_get_metadata_providers_result(&id, result);
        }
    }

    fn handle_get_metadata_providers_result(&mut self, id: &QueryId, result: GetProvidersResult) {
        match result {
            Ok(providers) => match providers {
                GetProvidersOk::FoundProviders {
                    key: _key,
                    providers,
                } => {
                    if let Some(data) = self.metadata_providers_requests.get_mut(&id) {
                        data.found_providers.extend(providers)
                    }
                }
                GetProvidersOk::FinishedWithNoAdditionalRecord { .. } => {
                    self.handle_all_possible_metadata_providers_found(&id);
                }
            },
            Err(error) => {
                error!(target: LOG_TARGET, "Error getting providers: {:?}", error);
                self.handle_all_possible_metadata_providers_found(&id);
            }
        }
    }

    fn handle_all_possible_metadata_providers_found(&mut self, id: &QueryId) {
        if let Some(data) = self.metadata_providers_requests.remove(&id) {
            if let Some(peer) = data.found_providers.iter().next() {
                let request_id = self
                    .swarm
                    .behaviour_mut()
                    .metadata_download
                    .send_request(peer, data.request);

                self.metadata_download_requests
                    .insert(request_id, data.result);
            } else if let Err(result) = data.result.send(None) {
                error!(target: LOG_TARGET, "Error calling oneshot channel to provide metadata response: {:?}", result);
            }
        }
    }

    async fn metadata_download(
        &mut self,
        event: request_response::Event<FileRequest, FileResponse>,
    ) {
        use request_response::Event::*;
        match event {
            Message {
                peer: _peer,
                connection_id: _connection_id,
                message,
            } => {
                use request_response::Message::*;
                match message {
                    Request {
                        request_id: _request_id,
                        request,
                        channel,
                    } => match self.store.get_published_file(request.file_id.into()).await {
                        Ok(Some(PublishedFileRecord {
                            key: _key,
                            original_file_name: _original_file_name,
                            target_dir,
                            public: _public,
                        })) => match tokio::fs::read(target_dir.join(METADATA_FILE_NAME)).await {
                            Ok(data) => self.send_metadata_download_response(
                                channel,
                                FileResponse::Success(data),
                            ),
                            Err(error) => self.send_metadata_download_response(
                                channel,
                                FileResponse::Error(error.to_string()),
                            ),
                        },
                        Ok(None) => {
                            self.send_metadata_download_response(channel, FileResponse::NotFound)
                        }
                        Err(error) => self.send_metadata_download_response(
                            channel,
                            FileResponse::Error(error.to_string()),
                        ),
                    },
                    Response {
                        request_id,
                        response,
                    } => {
                        if let Some(channel) = self.metadata_download_requests.remove(&request_id) {
                            self.handle_metadata_download_response(response, channel);
                        }
                    }
                }
            }
            _ => log_debug(&event),
        }
    }

    fn send_metadata_download_response(
        &mut self,
        channel: ResponseChannel<FileResponse>,
        file_response: FileResponse,
    ) {
        if let Err(error) = self
            .swarm
            .behaviour_mut()
            .metadata_download
            .send_response(channel, file_response)
        {
            error!(target: LOG_TARGET, "Error sending metadata download response: {:?}", error);
        }
    }

    fn handle_metadata_download_response(
        &mut self,
        response: FileResponse,
        channel: oneshot::Sender<Option<FileProcessingResult>>,
    ) {
        match response {
            FileResponse::Success(data) => {
                let result: Result<FileProcessingResult, serde_cbor::Error> = data.try_into();

                match result {
                    Ok(result) => {
                        if let Err(error) = channel.send(Some(result)) {
                            error!(target: LOG_TARGET, "Error sending metadata download response: {:?}", error);
                        }
                        return;
                    }
                    Err(error) => {
                        error!(target: LOG_TARGET, "Error deserializing response: {:?}", error);
                    }
                }
            }
            FileResponse::NotFound => {
                error!(target: LOG_TARGET, "Metadata download returned 404");
            }
            FileResponse::Error(error) => {
                error!(target: LOG_TARGET, "Error sending metadata download response: {:?}", error);
            }
        }
        if let Err(error) = channel.send(None) {
            error!(target: LOG_TARGET, "Error sending metadata download response: {:?}", error);
        }
    }

    fn file_download(&mut self, event: request_response::Event<FileRequest, FileResponse>) {
        use request_response::Event::*;
        match event {
            Message {
                peer: _peer,
                connection_id: _connection_id,
                message,
            } => {
                use request_response::Message::*;
                match message {
                    Request {
                        request_id: _request_id,
                        request,
                        channel: _channel,
                    } => {
                        info!(target: LOG_TARGET, "File download request: {:?}", request);
                    }
                    Response {
                        request_id: _request_id,
                        response,
                    } => {
                        info!(target: LOG_TARGET, "File download response: {:?}", response);
                    }
                }
            }
            _ => log_debug(&event),
        }
    }

    fn gossipsub(&mut self, event: gossipsub::Event) {
        use gossipsub::Event::*;
        match event {
            Message {
                propagation_source: _propagation_source,
                message_id: _message_id,
                message,
            } => {
                info!(target: LOG_TARGET, "[gossipsub] message: {:?}", message);
            }
            _ => log_debug(&event),
        }
    }

    fn mdns(&mut self, event: mdns::Event) {
        use mdns::Event::*;

        match event {
            Discovered(peers) => {
                for (peer_id, addr) in peers {
                    info!(target: LOG_TARGET, "[mDNS] Discovered {:?} at {:?}", peer_id, addr);

                    if is_dialable(&addr) {
                        self.swarm.add_peer_address(peer_id, addr.clone());
                    }

                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());

                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .add_explicit_peer(&peer_id);
                }
            }
            _ => log_debug(&event),
        }
    }

    fn identify(&mut self, event: identify::Event) {
        use identify::Event::*;

        match event {
            Received {
                connection_id: _,
                peer_id,
                info,
            } => {
                let is_relay = info
                    .protocols
                    .iter()
                    .any(|p| *p == relay::HOP_PROTOCOL_NAME);

                for addr in info.listen_addrs {
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());

                    if is_dialable(&addr) {
                        self.swarm.add_peer_address(peer_id, addr.clone());
                    }

                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .add_explicit_peer(&peer_id);

                    if is_relay {
                        if let Ok(relay_addr) = addr
                            .clone()
                            .with_p2p(peer_id)
                            .map(|a| a.with(Protocol::P2pCircuit))
                        {
                            info!(
                                target: LOG_TARGET,
                                "Listening via relay {}",
                                relay_addr
                            );

                            if let Err(e) = self.swarm.listen_on(relay_addr.clone()) {
                                warn!(
                                    target: LOG_TARGET,
                                    "Relay listen error on {}: {}",
                                    relay_addr,
                                    e
                                );
                            }
                        }
                    }
                }
            }
            _ => log_debug(&event),
        }
    }
}

fn log_debug<T: Debug>(event: &T) {
    debug!(target: LOG_TARGET, "{:?}", event);
}

fn is_dialable(addr: &libp2p::Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;

    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
            }
            _ => {}
        }
    }

    true
}
