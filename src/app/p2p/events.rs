use crate::app::file_processing::processing::FileMetadata;
use crate::app::file_store::domain::{
    PendingDownloadRecord, PublishedFileKey, PublishedFileRecord,
};
use crate::app::file_store::errors::FileStoreError;
use crate::app::file_store::FileStore;
use crate::app::p2p::config::P2pServiceConfig;
use crate::app::p2p::domain::{
    FileChunkRequest, FileFound, FileResponse, FileSearchRequest, FileSearchResult,
    MetadataFileRequest, P2pCommand, P2pNetworkBehaviour, P2pNetworkBehaviourEvent, PublishedFile,
};
use crate::app::p2p::errors::P2pNetworkError;
use crate::app::utils::{choose_random, METADATA_FILE_NAME};
use libp2p::futures::StreamExt;
use libp2p::gossipsub::IdentTopic;
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
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

const LOG_TARGET: &str = "app::p2p::events";

struct ProvidersRequestData<Req, Res> {
    found_providers: HashSet<PeerId>,
    request: Req,
    result: oneshot::Sender<Option<Res>>,
}

// possible a memory leak for maps caching queries and requests data
pub struct EventService<S: FileStore> {
    store: S,
    config: P2pServiceConfig,
    metadata_providers_requests:
        HashMap<QueryId, ProvidersRequestData<MetadataFileRequest, FileMetadata>>,
    file_chunk_providers_requests:
        HashMap<QueryId, ProvidersRequestData<FileChunkRequest, FileResponse>>,
    metadata_download_requests: HashMap<OutboundRequestId, oneshot::Sender<Option<FileMetadata>>>,
    file_download_requests: HashMap<OutboundRequestId, oneshot::Sender<Option<FileResponse>>>,
    file_search_sessions: HashMap<String, mpsc::Sender<FileFound>>,
}

impl<S: FileStore> EventService<S> {
    pub fn new(store: S, config: P2pServiceConfig) -> Self {
        Self {
            store,
            config,
            metadata_providers_requests: HashMap::new(),
            file_chunk_providers_requests: HashMap::new(),
            metadata_download_requests: HashMap::new(),
            file_download_requests: HashMap::new(),
            file_search_sessions: HashMap::new(),
        }
    }

    pub async fn handle_command(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        command: P2pCommand,
    ) {
        match command {
            P2pCommand::RequestMetadata { request, result } => {
                let key: PublishedFileKey = request.file_id.into();
                let key = RecordKey::new(&key.0);
                let query_id = swarm.behaviour_mut().kademlia.get_providers(key);
                self.metadata_providers_requests.insert(
                    query_id,
                    ProvidersRequestData {
                        found_providers: HashSet::new(),
                        request,
                        result,
                    },
                );
            }
            P2pCommand::RequestFileChunk { request, result } => {
                let key_bytes = match serde_cbor::to_vec(&request) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        error!(target: LOG_TARGET, "Error serializing file chunk request: {:?}", error);
                        if let Err(error) = result.send(None) {
                            error!(target: LOG_TARGET, "Error sending file chunk file response: {:?}", error);
                        }
                        return;
                    }
                };
                let key = RecordKey::new(&key_bytes);
                let query_id = swarm.behaviour_mut().kademlia.get_providers(key);
                self.file_chunk_providers_requests.insert(
                    query_id,
                    ProvidersRequestData {
                        found_providers: HashSet::new(),
                        request,
                        result,
                    },
                );
            }
            P2pCommand::ProvideMetadata(metadata) => {
                if let Err(error) = self.provide_metadata(swarm, &metadata).await {
                    error!(target: LOG_TARGET, "failed to provide metadata: {}", error);
                }
            }
            P2pCommand::ProvideFileChunk(request) => {
                if let Err(error) = self.provide_file_chunk(swarm, &request).await {
                    error!(target: LOG_TARGET, "failed to provide file chunk: {}", error);
                }
            }
            P2pCommand::FileSearch(original_request, tx) => {
                if let Some(topic) = &self.config.file_search_topic {
                    let session_id = original_request.session_id.clone();
                    let request: Result<Vec<u8>, serde_cbor::Error> =
                        original_request.try_into();

                    match request {
                        Ok(bytes) => {
                            if let Err(error) = swarm
                                .behaviour_mut()
                                .gossipsub
                                .publish(IdentTopic::new(topic.clone()), bytes)
                            {
                                error!(target: LOG_TARGET, "failed to search for file: {:?}", error);
                            } else {
                                self.file_search_sessions.insert(session_id, tx);
                            }
                        }
                        Err(error) => {
                            error!(target: LOG_TARGET, "failed to search for file: {:?}", error);
                        }
                    }
                } else {
                    error!(target: LOG_TARGET, "failed to get file search topic");
                }
            }
            P2pCommand::FileSearchAbort(session_id) => {
                self.file_search_sessions.remove(&session_id);
            }
            P2pCommand::FileSearchResult(peer_id, file_search_result) => {
                swarm
                    .behaviour_mut()
                    .file_search_results
                    .send_request(&peer_id, file_search_result);
            }
        }
    }

    async fn provide_metadata(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        metadata: &FileMetadata,
    ) -> Result<(), P2pNetworkError> {
        let raw_key = metadata.key();

        let value = serde_cbor::to_vec(&PublishedFile {
            total_chunks: metadata.total_chunks,
            merkle_root: metadata.merkle_root,
        })?;

        let record = Record::new(raw_key.to_vec(), value);
        let record_key = record.key.clone();

        swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, Quorum::Majority)?;

        swarm.behaviour_mut().kademlia.start_providing(record_key)?;

        info!(target: LOG_TARGET, "Provided record key: {:?}", u64::from(PublishedFileKey(raw_key)));

        Ok(())
    }

    async fn provide_file_chunk(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        file_chunk: &FileChunkRequest,
    ) -> Result<(), P2pNetworkError> {
        let value = serde_cbor::to_vec(file_chunk)?;

        let record = Record::new(value.to_vec(), value);
        let record_key = record.key.clone();

        swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, Quorum::Majority)?;

        swarm.behaviour_mut().kademlia.start_providing(record_key)?;

        Ok(())
    }

    async fn provide_all(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        metadata: &FileMetadata,
    ) -> Result<(), P2pNetworkError> {
        self.provide_metadata(swarm, metadata).await?;
        let file_id: u64 = u64::from(PublishedFileKey(metadata.key().clone()));
        for chunk_id in 0..metadata.total_chunks {
            self.provide_file_chunk(swarm, &FileChunkRequest { file_id, chunk_id })
                .await?;
        }
        Ok(())
    }

    pub async fn provide_all_published_files(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
    ) -> Result<(), P2pNetworkError> {
        let mut receiver_stream = self.store.stream_published_files();

        while let Some(result) = receiver_stream.next().await {
            let published_file = result?;
            let metadata_buf = published_file.chunks_dir.join(METADATA_FILE_NAME);
            match tokio::fs::read(metadata_buf).await {
                Ok(data) => self.provide_all(swarm, &data.try_into()?).await?,
                Err(error) => error!(target: LOG_TARGET, "Error reading metadata file: {}", error),
            };
        }

        Ok(())
    }

    pub async fn file_publish(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        result: Result<FileMetadata, FileStoreError>,
    ) {
        match result {
            Ok(metadata) => {
                while let Err(error) = self.provide_all(swarm, &metadata).await {
                    error!(target: LOG_TARGET, "Error providing file: {}", error);
                    sleep(Duration::from_secs(1)).await
                }

                let raw_key = metadata.key();
                self.add_published_file(metadata).await;
                self.delete_file_metadata(raw_key).await;
                info!(target: LOG_TARGET, "Successfully published a file");
            }
            Err(error) => {
                error!(target: LOG_TARGET, "File store error: {:?}", error);
            }
        }
    }

    async fn add_published_file(&mut self, metadata: FileMetadata) {
        let published_file_record: PublishedFileRecord = metadata.into();
        while let Err(error) = self
            .store
            .put_published_file(published_file_record.clone())
            .await
        {
            error!(target: LOG_TARGET, "Failed to add published file: {}", error);
            sleep(Duration::from_secs(1)).await
        }
    }

    async fn delete_file_metadata(&mut self, metadata_key: [u8; 8]) {
        loop {
            match self.store.delete_file_metadata(metadata_key.clone()).await {
                Err(error) => {
                    error!(target: LOG_TARGET, "Error deleting file split record: {}", error);
                    sleep(Duration::from_secs(1)).await
                }
                _ => break,
            }
        }
    }

    pub async fn handle_swarm_event(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        tx: mpsc::Sender<P2pCommand>,
        event: SwarmEvent<P2pNetworkBehaviourEvent>,
    ) {
        use P2pNetworkBehaviourEvent::*;
        use SwarmEvent::*;
        match event {
            Behaviour(event) => match event {
                Identify(event) => self.identify(swarm, event),
                Mdns(event) => self.mdns(swarm, event),
                Kademlia(event) => self.kademlia(swarm, event),
                Gossipsub(event) => self.gossipsub(tx.clone(), event),
                RelayServer(event) => log_debug(&event),
                RelayClient(event) => log_debug(&event),
                Dcutr(event) => log_debug(&event),
                FileDownload(event) => self.file_download(swarm, event).await,
                MetadataDownload(event) => self.metadata_download(swarm, event).await,
                FileSearchResults(event) => self.file_search_results(swarm, event).await,
                _ => log_debug(&event),
            },
            NewListenAddr {
                listener_id: _listener_id,
                address,
            } => info!(target: LOG_TARGET, "Listening on {:?}", address),
            _ => log_debug(&event),
        }
    }

    async fn file_search_results(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        event: request_response::Event<FileSearchResult, ()>,
    ) {
        use request_response::Event::*;
        match event {
            Message {
                peer: _peer_id,
                connection_id: _connection_id,
                message,
            } => {
                use request_response::Message::*;
                match message {
                    Request {
                        request_id: _request_id,
                        request,
                        channel,
                    } => {
                        let _ = swarm
                            .behaviour_mut()
                            .file_search_results
                            .send_response(channel, ());

                        if let Some(tx) = self.file_search_sessions.get(&request.session_id) {
                            if let Err(_) = tx
                                .send(FileFound {
                                    file_id: request.file_found.file_id,
                                    file_name: request.file_found.file_name.clone(),
                                })
                                .await
                            {
                                self.file_search_sessions.remove(&request.session_id);
                            }
                        }
                    }
                    Response { .. } => {}
                }
            }
            _ => {}
        }
    }

    fn kademlia(&mut self, swarm: &mut Swarm<P2pNetworkBehaviour>, event: kad::Event) {
        use kad::Event::*;
        match event {
            OutboundQueryProgressed {
                id,
                result,
                stats: _stats,
                step: _step,
            } => {
                self.handle_metadata_providers_query_progressed(swarm, id, result);
            }
            _ => log_debug(&event),
        }
    }

    fn handle_metadata_providers_query_progressed(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        id: QueryId,
        result: QueryResult,
    ) {
        if let QueryResult::GetProviders(result) = result {
            self.handle_get_metadata_providers_result(swarm, &id, result);
        }
    }

    fn handle_get_metadata_providers_result(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        id: &QueryId,
        result: GetProvidersResult,
    ) {
        match result {
            Ok(providers) => match providers {
                GetProvidersOk::FoundProviders {
                    key: _key,
                    providers,
                } => {
                    if let Some(data) = self.metadata_providers_requests.get_mut(&id) {
                        data.found_providers.extend(providers)
                    } else if let Some(data) = self.file_chunk_providers_requests.get_mut(&id) {
                        data.found_providers.extend(providers)
                    }
                }
                GetProvidersOk::FinishedWithNoAdditionalRecord { .. } => {
                    self.handle_all_possible_providers_found(swarm, &id);
                }
            },
            Err(error) => {
                error!(target: LOG_TARGET, "Error getting providers: {:?}", error);
                self.handle_all_possible_providers_found(swarm, &id);
            }
        }
    }

    fn handle_all_possible_providers_found(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        id: &QueryId,
    ) {
        if let Some(data) = self.metadata_providers_requests.remove(&id) {
            if let Some(peer) = choose_random(&data.found_providers) {
                let request_id = swarm
                    .behaviour_mut()
                    .metadata_download
                    .send_request(peer, data.request);

                self.metadata_download_requests
                    .insert(request_id, data.result);
            } else if let Err(result) = data.result.send(None) {
                error!(target: LOG_TARGET, "Error calling oneshot channel to provide metadata response: {:?}", result);
            }
        } else if let Some(data) = self.file_chunk_providers_requests.remove(&id) {
            if let Some(peer) = choose_random(&data.found_providers) {
                let request_id = swarm
                    .behaviour_mut()
                    .file_download
                    .send_request(peer, data.request);

                self.file_download_requests.insert(request_id, data.result);
            } else if let Err(result) = data.result.send(None) {
                error!(target: LOG_TARGET, "Error calling oneshot channel to provide metadata response: {:?}", result);
            }
        }
    }

    async fn metadata_download(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        event: request_response::Event<MetadataFileRequest, FileResponse>,
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
                            chunks_dir: target_dir,
                            public: _public,
                        })) => match tokio::fs::read(target_dir.join(METADATA_FILE_NAME)).await {
                            Ok(data) => {
                                self.send_file_response(swarm, channel, FileResponse::Success(data))
                            }
                            Err(error) => self.send_file_response(
                                swarm,
                                channel,
                                FileResponse::Error(error.to_string()),
                            ),
                        },
                        Ok(None) => self.send_file_response(swarm, channel, FileResponse::NotFound),
                        Err(error) => self.send_file_response(
                            swarm,
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

    fn send_file_response(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        channel: ResponseChannel<FileResponse>,
        file_response: FileResponse,
    ) {
        if let Err(error) = swarm
            .behaviour_mut()
            .metadata_download
            .send_response(channel, file_response)
        {
            error!(target: LOG_TARGET, "Error sending metadata download response: {:?}", error);
        }
    }

    fn handle_file_download_response(
        &mut self,
        response: FileResponse,
        channel: oneshot::Sender<Option<FileResponse>>,
    ) {
        if let Err(error) = channel.send(Some(response)) {
            error!(target: LOG_TARGET, "Error sending file download response: {:?}", error);
        }
    }

    fn handle_metadata_download_response(
        &mut self,
        response: FileResponse,
        channel: oneshot::Sender<Option<FileMetadata>>,
    ) {
        match response {
            FileResponse::Success(data) => {
                let result: Result<FileMetadata, serde_cbor::Error> = data.try_into();

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

    async fn file_download(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        event: request_response::Event<FileChunkRequest, FileResponse>,
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
                        request: FileChunkRequest { file_id, chunk_id },
                        channel,
                    } => {
                        info!(target: LOG_TARGET, "File download request: file id {}, chunk id {}", file_id, chunk_id);

                        match self.store.get_published_file(file_id.into()).await {
                            Ok(None) => {
                                self.try_from_pending_downloads(swarm, file_id, chunk_id, channel)
                                    .await
                            }
                            Ok(Some(PublishedFileRecord {
                                key: _key,
                                original_file_name: _original_file_name,
                                chunks_dir: target_dir,
                                public: _public,
                            })) => {
                                self.get_from_chunks(swarm, chunk_id, channel, target_dir)
                                    .await
                            }
                            Err(error) => {
                                error!(target: LOG_TARGET, "Error getting published file: {:?}", error);
                                self.send_file_response(
                                    swarm,
                                    channel,
                                    FileResponse::Error(error.to_string()),
                                );
                            }
                        };
                    }
                    Response {
                        request_id,
                        response,
                    } => {
                        if let Some(channel) = self.file_download_requests.remove(&request_id) {
                            self.handle_file_download_response(response, channel);
                        }
                    }
                }
            }
            _ => log_debug(&event),
        }
    }

    async fn get_from_chunks(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        chunk_id: usize,
        channel: ResponseChannel<FileResponse>,
        target_dir: PathBuf,
    ) {
        let metadata: FileMetadata = match tokio::fs::read(target_dir.join(METADATA_FILE_NAME))
            .await
        {
            Ok(bytes) => match bytes.try_into() {
                Ok(metadata) => metadata,
                Err(error) => {
                    error!(target: LOG_TARGET, "Error deserializing metadata from file: {:?}", error);
                    self.send_file_response(swarm, channel, FileResponse::Error(error.to_string()));
                    return;
                }
            },
            Err(error) => {
                error!(target: LOG_TARGET, "Error reading metadata file: {:?}", error);
                self.send_file_response(swarm, channel, FileResponse::Error(error.to_string()));
                return;
            }
        };

        let FileMetadata {
            original_file_name: _original_file_name,
            total_chunks,
            chunks_dir: target_dir,
            merkle_root: _merkle_root,
            merkle_proofs: _merkle_proofs,
            chunk_file_extension,
            public: _public,
        } = metadata;

        if chunk_id >= total_chunks {
            self.send_file_response(
                swarm,
                channel,
                FileResponse::Error(format!("incorrect chunk number: {}", chunk_id)),
            );
            return;
        }

        self.read_chunk(swarm, chunk_id, channel, target_dir, chunk_file_extension)
            .await;
    }

    async fn read_chunk(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        chunk_id: usize,
        channel: ResponseChannel<FileResponse>,
        target_dir: PathBuf,
        chunk_file_extension: String,
    ) {
        match tokio::fs::read(target_dir.join(format!("{}.{}", chunk_id, chunk_file_extension)))
            .await
        {
            Ok(bytes) => self.send_file_response(swarm, channel, FileResponse::Success(bytes)),
            Err(error) => {
                error!(target: LOG_TARGET, "Error reading chunk file: {:?}", error);
                self.send_file_response(swarm, channel, FileResponse::Error(error.to_string()))
            }
        }
    }

    async fn try_from_pending_downloads(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        file_id: u64,
        chunk_id: usize,
        channel: ResponseChannel<FileResponse>,
    ) {
        match self.store.get_pending_download(file_id.into()).await {
            Ok(None) => self.send_file_response(swarm, channel, FileResponse::NotFound),
            Ok(Some(PendingDownloadRecord {
                key: _key,
                original_file_name: _original_file_name,
                chunks_dir: download_path,
                downloaded_chunks,
            })) => {
                if downloaded_chunks.contains(&chunk_id) {
                    self.get_from_chunks(swarm, chunk_id, channel, download_path)
                        .await;
                } else {
                    self.send_file_response(swarm, channel, FileResponse::NotFound);
                }
            }
            Err(error) => {
                error!(target: LOG_TARGET, "Error finding pending download: {:?}", error);
                self.send_file_response(swarm, channel, FileResponse::Error(error.to_string()));
            }
        }
    }

    fn gossipsub(&mut self, tx: mpsc::Sender<P2pCommand>, event: gossipsub::Event) {
        use gossipsub::Event::*;
        match event {
            Message {
                propagation_source,
                message_id: _message_id,
                message,
            } => {
                if let Some(topic) = &self.config.file_search_topic {
                    if message.topic == IdentTopic::new(topic).hash() {
                        let request: Result<FileSearchRequest, serde_cbor::Error> =
                            message.data.try_into();
                        match request {
                            Ok(request) => {
                                let commands = tx.clone();
                                let store = self.store.clone();
                                tokio::spawn(async move {
                                    let mut stream = store.stream_published_files();
                                    while let Some(result) = stream.next().await {
                                        match result {
                                            Ok(published_file_record) => {
                                                if published_file_record.public
                                                    && published_file_record
                                                        .original_file_name
                                                        .contains(&request.search_value)
                                                {
                                                    if let Err(error) = commands
                                                        .send(P2pCommand::FileSearchResult(
                                                            propagation_source.clone(),
                                                            FileSearchResult {
                                                                session_id: request
                                                                    .session_id
                                                                    .clone(),
                                                                file_found: FileFound {
                                                                    file_id: published_file_record
                                                                        .key
                                                                        .into(),
                                                                    file_name:
                                                                        published_file_record
                                                                            .original_file_name,
                                                                },
                                                            },
                                                        ))
                                                        .await
                                                    {
                                                        error!(target: LOG_TARGET, "Error sending P2pCommand: {:?}", error);
                                                        break;
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                error!(target: LOG_TARGET, "Error reading published file: {:?}", error);
                                                break;
                                            }
                                        }
                                    }
                                });
                            }
                            Err(error) => {
                                error!(target: LOG_TARGET, "Error deserializing message: {:?}", error);
                            }
                        }
                    }
                }
            }
            _ => log_debug(&event),
        }
    }

    fn mdns(&mut self, swarm: &mut Swarm<P2pNetworkBehaviour>, event: mdns::Event) {
        use mdns::Event::*;

        match event {
            Discovered(peers) => {
                for (peer_id, addr) in peers {
                    info!(target: LOG_TARGET, "[mDNS] Discovered {:?} at {:?}", peer_id, addr);

                    if is_dialable(&addr) {
                        swarm.add_peer_address(peer_id, addr.clone());
                    }

                    swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());

                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                }
            }
            _ => log_debug(&event),
        }
    }

    fn identify(&mut self, swarm: &mut Swarm<P2pNetworkBehaviour>, event: identify::Event) {
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
                    swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, addr.clone());

                    if is_dialable(&addr) {
                        swarm.add_peer_address(peer_id, addr.clone());
                    }

                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

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

                            if let Err(e) = swarm.listen_on(relay_addr.clone()) {
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
