use crate::app::file_processing::processing::FileProcessingResult;
use crate::app::file_store::domain::{
    PendingDownloadRecord, PublishedFileKey, PublishedFileRecord,
};
use crate::app::file_store::errors::FileStoreError;
use crate::app::file_store::FileStore;
use crate::app::p2p::domain::{
    DownloadFileChunk, FileChunkRequest, FileResponse, MetadataFileRequest, P2pCommand,
    P2pNetworkBehaviour, P2pNetworkBehaviourEvent, PublishedFile,
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex, Semaphore};
use tokio::task::JoinSet;
use tokio::time::sleep;

const LOG_TARGET: &str = "app::p2p::events";

pub struct MetadataProvidersRequestData {
    found_providers: HashSet<PeerId>,
    request: MetadataFileRequest,
    result: oneshot::Sender<Option<FileProcessingResult>>,
}

// possible a memory leak for maps caching queries and requests data
pub struct EventService<S: FileStore> {
    store: S,
    metadata_providers_requests: HashMap<QueryId, MetadataProvidersRequestData>,
    metadata_download_requests:
        HashMap<OutboundRequestId, oneshot::Sender<Option<FileProcessingResult>>>,
    active_downloads: Arc<Mutex<HashSet<PublishedFileKey>>>,
    active_downloads_semaphore: Arc<Semaphore>,
}

impl<S: FileStore> EventService<S> {
    pub fn new(store: S, max_active_downloads: u16) -> Self {
        Self {
            store,
            metadata_providers_requests: HashMap::new(),
            metadata_download_requests: HashMap::new(),
            active_downloads: Arc::new(Mutex::new(HashSet::new())),
            active_downloads_semaphore: Arc::new(Semaphore::new(max_active_downloads as usize)),
        }
    }

    async fn download<FS: FileStore>(
        pending_download_record: &mut PendingDownloadRecord,
        semaphore: Arc<Semaphore>,
        store: FS,
    ) -> anyhow::Result<bool> {
        let file_processing_result: FileProcessingResult = tokio::fs::read(
            pending_download_record
                .download_path
                .join(METADATA_FILE_NAME),
        )
        .await?
        .try_into()?;

        let mut join_set = JoinSet::new();
        for chunk_id in 0..file_processing_result.merkle_proofs.len() {
            if pending_download_record
                .downloaded_chunks
                .contains(&chunk_id)
            {
                continue;
            }

            let download_file_chunk = DownloadFileChunk {
                chunk_id,
                merkle_root: file_processing_result.merkle_root.clone(),
                merkle_proof: file_processing_result.merkle_proofs[chunk_id].clone(),
            };

            let local_semaphore = semaphore.clone();

            join_set.spawn(tokio::spawn(async move {
                let permit = local_semaphore.acquire().await?;
                debug!("permit acquired {:?}", permit);
                let result = Self::download_file_chunk(download_file_chunk).await;
                debug!("permit released {:?}", permit);
                result
            }));
        }

        let mut first_error = None;
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(result) => match result {
                    Ok(result) => match result {
                        Ok(chunk_id) => {
                            info!(target: LOG_TARGET, "{}: chunk {} has been downloaded", &file_processing_result.original_file_name, chunk_id);
                            if pending_download_record.downloaded_chunks.insert(chunk_id) {
                                if let Err(error) = store
                                    .add_pending_download(pending_download_record.clone())
                                    .await
                                {
                                    error!("failed to update pending download: {}", error);
                                }
                            }
                        }
                        Err(error) => {
                            error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &file_processing_result.original_file_name, error);
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        }
                    },
                    Err(error) => {
                        error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &file_processing_result.original_file_name, error);
                        if first_error.is_none() {
                            first_error = Some(error.into());
                        }
                    }
                },
                Err(error) => {
                    error!(target: LOG_TARGET, "{}: failed to download chunk: {}", &file_processing_result.original_file_name, error);
                    if first_error.is_none() {
                        first_error = Some(error.into());
                    }
                }
            }
        }

        first_error
            .map(Err)
            .unwrap_or(Ok(pending_download_record.downloaded_chunks.len()
                == file_processing_result.merkle_proofs.len()))
    }

    async fn download_file_chunk(download_file_chunk: DownloadFileChunk) -> anyhow::Result<usize> {
        sleep(Duration::from_secs(1)).await;
        Ok(download_file_chunk.chunk_id)
    }

    pub async fn work_on_pending_downloads(&mut self) {
        if self.active_downloads_semaphore.available_permits() == 0 {
            info!(target: LOG_TARGET, "no download permits available");
            return;
        }
        let mut stream = self.store.stream_pending_downloads();
        while let Some(result) = stream.next().await {
            if self.active_downloads_semaphore.available_permits() == 0 {
                info!(target: LOG_TARGET, "no download permits available");
                return;
            }
            match result {
                Ok(mut pending_download_record) => {
                    let active_downloads = self.active_downloads.clone();

                    if active_downloads
                        .lock()
                        .await
                        .insert(pending_download_record.key.clone())
                    {
                        let semaphore = self.active_downloads_semaphore.clone();
                        let store = self.store.clone();
                        tokio::spawn(async move {
                            match Self::download(&mut pending_download_record, semaphore, store)
                                .await
                            {
                                Ok(completed) => {
                                    if completed {
                                        // todo!(
                                        //     "restore original file, remove from pending downloads and start providing the downloaded data"
                                        // );
                                    }
                                }
                                Err(error) => {
                                    error!(target: LOG_TARGET, "Failed to start download: {}", error);
                                }
                            }
                            active_downloads
                                .lock()
                                .await
                                .remove(&pending_download_record.key);
                        });
                    }
                }
                Err(error) => {
                    error!(target: LOG_TARGET, "Error reading from stream: {}", error);
                }
            }
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
                    MetadataProvidersRequestData {
                        found_providers: HashSet::new(),
                        request,
                        result,
                    },
                );
            }
            P2pCommand::RequestFileChunk { request, result } => {
                // TODO
            }
        }
    }

    async fn provide(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        file_processing_result: &FileProcessingResult,
    ) -> Result<(), P2pNetworkError> {
        let raw_key = file_processing_result.key();

        let value = serde_cbor::to_vec(&PublishedFile {
            total_chunks: file_processing_result.total_chunks,
            merkle_root: file_processing_result.merkle_root,
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

    pub async fn provide_all_published_files(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
    ) -> Result<(), P2pNetworkError> {
        let mut receiver_stream = self.store.stream_published_files();

        while let Some(result) = receiver_stream.next().await {
            let published_file = result?;
            let metadata_buf = published_file.target_dir.join(METADATA_FILE_NAME);
            match tokio::fs::read(metadata_buf).await {
                Ok(data) => self.provide(swarm, &data.try_into()?).await?,
                Err(error) => error!(target: LOG_TARGET, "Error reading metadata file: {}", error),
            };
        }

        Ok(())
    }

    pub async fn file_publish(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        result: Result<FileProcessingResult, FileStoreError>,
    ) {
        match result {
            Ok(file_processing_result) => {
                while let Err(error) = self.provide(swarm, &file_processing_result).await {
                    error!(target: LOG_TARGET, "Error providing file: {}", error);
                    sleep(Duration::from_secs(1)).await
                }

                let raw_key = file_processing_result.key();
                self.add_published_file(file_processing_result).await;
                self.delete_file_processing_result(raw_key).await;
                info!(target: LOG_TARGET, "Successfully published a file");
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
            error!(target: LOG_TARGET, "Failed to add published file: {}", error);
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

    pub async fn handle_swarm_event(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        event: SwarmEvent<P2pNetworkBehaviourEvent>,
    ) {
        use P2pNetworkBehaviourEvent::*;
        use SwarmEvent::*;
        match event {
            Behaviour(event) => match event {
                Identify(event) => self.identify(swarm, event),
                Mdns(event) => self.mdns(swarm, event),
                Kademlia(event) => self.kademlia(swarm, event),
                Gossipsub(event) => self.gossipsub(event),
                RelayServer(event) => log_debug(&event),
                RelayClient(event) => log_debug(&event),
                Dcutr(event) => log_debug(&event),
                FileDownload(event) => self.file_download(swarm, event).await,
                MetadataDownload(event) => self.metadata_download(swarm, event).await,
                _ => log_debug(&event),
            },
            NewListenAddr {
                listener_id: _listener_id,
                address,
            } => info!(target: LOG_TARGET, "Listening on {:?}", address),
            _ => log_debug(&event),
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
                    }
                }
                GetProvidersOk::FinishedWithNoAdditionalRecord { .. } => {
                    self.handle_all_possible_metadata_providers_found(swarm, &id);
                }
            },
            Err(error) => {
                error!(target: LOG_TARGET, "Error getting providers: {:?}", error);
                self.handle_all_possible_metadata_providers_found(swarm, &id);
            }
        }
    }

    fn handle_all_possible_metadata_providers_found(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        id: &QueryId,
    ) {
        if let Some(data) = self.metadata_providers_requests.remove(&id) {
            if let Some(peer) = data.found_providers.iter().next() {
                let request_id = swarm
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
                            target_dir,
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
                                target_dir,
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

    async fn get_from_chunks(
        &mut self,
        swarm: &mut Swarm<P2pNetworkBehaviour>,
        chunk_id: usize,
        channel: ResponseChannel<FileResponse>,
        target_dir: PathBuf,
    ) {
        let file_processing_result: FileProcessingResult = match tokio::fs::read(
            target_dir.join(METADATA_FILE_NAME),
        )
        .await
        {
            Ok(bytes) => match bytes.try_into() {
                Ok(file_processing_result) => file_processing_result,
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

        let FileProcessingResult {
            original_file_name: _original_file_name,
            total_chunks,
            target_dir,
            merkle_root: _merkle_root,
            merkle_proofs: _merkle_proofs,
            chunk_file_extension,
            public: _public,
        } = file_processing_result;

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
                download_path,
                downloaded_chunks,
            })) => {
                if downloaded_chunks.contains(&chunk_id) {
                    self.get_from_chunks(swarm, chunk_id, channel, download_path).await;
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
