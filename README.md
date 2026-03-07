# Rust DFS

Rust DFS is an experimental distributed file system node written in
Rust.

It allows peers to publish files, search for files in the network, and
download them from other nodes.\
The node exposes a gRPC API for clients and communicates with other
peers using libp2p.

This project is intended as an experimental implementation for learning
and exploring distributed systems in Rust.

------------------------------------------------------------------------

## Features

-   Distributed file publishing
-   File search across peers
-   Peer-to-peer file downloads
-   gRPC API for clients
-   libp2p networking
-   RocksDB metadata storage
-   Concurrent downloads
-   Local file metadata processing
-   Caching layer

------------------------------------------------------------------------

## Architecture

    +--------------------+
    |      CLI / API     |
    |       (gRPC)       |
    +----------+---------+
               |
               v
    +--------------------+
    |    Application     |
    |      Service       |
    +----------+---------+
               |
               +----------------------+
               |                      |
               v                      v
    +-------------------+   +-------------------+
    |     File Store    |   |        P2P        |
    |    (RocksDB)      |   |      libp2p       |
    +-------------------+   +-------------------+
               |
               v
    +-------------------+
    |   Local Storage   |
    +-------------------+

------------------------------------------------------------------------

## Main Components

### gRPC Server

The node exposes a gRPC interface for external clients.

Main operations:

-   publish files
-   search files
-   download files

Implementation:

    src/app/grpc

------------------------------------------------------------------------

### P2P Network

Peer-to-peer communication between nodes is implemented using
**libp2p**.

Responsibilities:

-   peer discovery
-   search requests
-   file metadata exchange

Implementation:

    src/app/p2p

------------------------------------------------------------------------

### File Store

File metadata is stored locally using **RocksDB**.

Responsibilities:

-   file index
-   published file records
-   pending downloads

Implementation:

    src/app/file_store

------------------------------------------------------------------------

### File Processing

Handles file metadata extraction and preparation before publishing.

Implementation:

    src/app/file_processing

------------------------------------------------------------------------

### Download Service

Manages concurrent downloads and download scheduling.

Implementation:

    src/app/file_download_service.rs

------------------------------------------------------------------------

## gRPC API

Defined in:

    proto/dfs.proto

### PublishFile

Registers a file in the network.

Request:

    file_path
    public

Response:

    file_id

------------------------------------------------------------------------

### DownloadFile

Downloads a file from peers.

Request:

    file_id
    download_path

------------------------------------------------------------------------

### Search

Streams search results.

Request:

    search_value

Response stream:

    file_id
    file_name

------------------------------------------------------------------------

## Installation

Clone the repository:

``` bash
git clone https://github.com/rusmemes/rust-dfs.git
cd rust-dfs
```

Build the project:

``` bash
cargo build --release
```

------------------------------------------------------------------------

## Running a Node

Start a DFS node:

``` bash
cargo run -- start
```

------------------------------------------------------------------------

## CLI Options

  Option                     Description
  -------------------------- --------------------------------
  `--base-path`              Base directory for DFS data
  `--grpc-port`              Port for gRPC server
  `--max-active-downloads`   Maximum simultaneous downloads
  `--file-search-topic`      P2P topic used for search

Example:

``` bash
cargo run -- start \
  --grpc-port 9999 \
  --file-search-topic file-search \
  --max-active-downloads 10
```

------------------------------------------------------------------------

## Project Structure

    src
     └ app
        ├ cli
        ├ errors
        ├ file_processing
        ├ file_store
        ├ grpc
        ├ p2p
        ├ server.rs
        └ utils.rs

    proto
     └ dfs.proto

------------------------------------------------------------------------

## Development

Run with logging:

``` bash
RUST_LOG=info cargo run -- start
```

------------------------------------------------------------------------

## Project Status

This project is experimental and under active development.

APIs and architecture may change.

------------------------------------------------------------------------

## License

MIT
