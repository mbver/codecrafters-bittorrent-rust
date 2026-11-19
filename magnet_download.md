```rs
impl Client {
    pub fn new(torrent: Torrent) -> io::Result<Self> {
        // Existing implementation.
    }

    pub fn new_from_magnet(magnet: &str) -> io::Result<Self> {
        // Obtain Torrent from magnet metadata.
        // Then construct Client.
    }
}
```

```rs
impl Client {
    fn new_with_peers(
        torrent: Torrent,
        peers: Vec<SocketAddr>,
    ) -> Self {
        let mut client_id = [0u8; 20];
        rand::rng().fill(&mut client_id);

        Self {
            left: torrent.length,
            torrent,
            client_id,
            peers: Mutex::new(peers.into()),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
        }
    }
}
```

```rs
pub fn new(torrent: Torrent) -> io::Result<Self> {
    let client = Self::new_with_peers(torrent, Vec::new());

    let peers = client.fetch_peers()?;
    client.peers.lock().unwrap().extend(peers);

    Ok(client)
}
```

```rs
pub fn new_from_magnet(magnet: &str) -> io::Result<Self> {
    let mut peer = manget_metdata_peer(magnet)?;
    let metadata = request_metadata(&mut peer)?;

    let torrent = Torrent::from_metadata(metadata)?;

    let client = Self::new_with_peers(torrent, Vec::new());

    let peers = client.fetch_peers()?;
    client.peers.lock().unwrap().extend(peers);

    Ok(client)
}
```

```rs
"magnet_download_piece" => {
    let output = Path::new(&args[3]);
    let magnet = &args[4];
    let piece_index = args[5].parse()?;

    let client = Client::new_from_magnet(magnet)?;
    client.download_single_piece(output, piece_index)?;
}
```
