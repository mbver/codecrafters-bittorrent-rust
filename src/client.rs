use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::{fs, thread};
use std::{
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::Path,
};

const BLOCK_SIZE: usize = 16 * 1024;
const MAX_WORKERS: usize = 8;
const RETRY_DELAY_MIN: Duration = Duration::from_millis(500);
const RETRY_DELAY_MAX: Duration = Duration::from_millis(1500);
const EXTENSION_MESSAGE_ID: u8 = 20;
const EXTENSION_HANDSHAKE_ID: u8 = 0;
const METADATA_REQUEST: u8 = 0;
const METADATA_DATA: i64 = 1;
const METADATA_REJECT: i64 = 2;

use rand::RngExt;
use reqwest::Url;
use sha1::{Digest, Sha1};

use crate::{
    bencode::{BencodeValue, decode_bencoded_value},
    message::Message,
    torrent::Torrent,
};

pub struct Client {
    torrent: Torrent,
    client_id: [u8; 20],
    peers: Mutex<VecDeque<SocketAddr>>,
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
}

pub struct Peer {
    pub peer_id: [u8; 20],
    pub addr: SocketAddr,
    pub stream: TcpStream,
    pub extensions: Option<HashMap<String, u8>>,
}

/// Everything we need to know about a piece before downloading it.
struct PieceInfo {
    /// Actual length of this piece (the last piece may be shorter).
    length: usize,
    expected_hash: [u8; 20],
}

impl Client {
    pub fn new(torrent: Torrent) -> io::Result<Self> {
        let mut client_id = [0u8; 20];
        rand::rng().fill(&mut client_id);

        let client = Self {
            left: torrent.length,
            torrent,
            client_id,
            peers: Mutex::new(VecDeque::new()),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
        };
        {
            let mut peers = client.peers.lock().unwrap();
            for peer in client.fetch_peers()? {
                peers.push_back(peer);
            }
        }

        Ok(client)
    }
    pub fn fetch_peers(&self) -> io::Result<Vec<SocketAddr>> {
        fetch_peers(
            &self.torrent.announce,
            &self.torrent.info_hash,
            &self.client_id,
            self.port,
            self.uploaded,
            self.downloaded,
            self.left,
        )
    }
    pub fn handshake(
        &self,
        peer_addr: SocketAddr,
        extensions: Option<&[(&str, u8)]>,
    ) -> io::Result<Peer> {
        handshake(
            peer_addr,
            &self.torrent.info_hash,
            &self.client_id,
            extensions,
        )
    }

    pub fn find_next_active_peer(&self) -> io::Result<Peer> {
        let n = self.peers.lock().unwrap().len();
        let mut last_error = None;

        for _ in 0..n {
            let addr = {
                let mut peers = self.peers.lock().unwrap();
                let Some(addr) = peers.pop_front() else { break };
                peers.push_back(addr);
                addr
            }; // lock dropped here, before the blocking connect/handshake

            match self.handshake(addr, None) {
                Ok(peer) => return Ok(peer),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no peers available")))
    }

    fn get_piece_info(&self, piece_index: usize) -> io::Result<PieceInfo> {
        let mut piece_length = usize::try_from(self.torrent.piece_length)
            .map_err(|_| io::Error::other("piece length too large"))?;
        let total_length = usize::try_from(self.torrent.length)
            .map_err(|_| io::Error::other("torrent length too large"))?;

        let piece_offset = piece_index
            .checked_mul(piece_length)
            .ok_or_else(|| io::Error::other("piece offset overflow"))?;
        if piece_offset >= total_length {
            return Err(io_invalid_data("piece_offset exceeds total length"));
        }
        piece_length = piece_length.min(total_length - piece_offset);
        if piece_length == 0 {
            return Err(io_invalid_data("piece_length is zero."));
        }

        let hash_start = piece_index
            .checked_mul(20)
            .ok_or_else(|| io::Error::other("hash offset overflow"))?;
        let hash_end = hash_start
            .checked_add(20)
            .ok_or_else(|| io::Error::other("hash offset overflow"))?;

        let expected_hash = self
            .torrent
            .pieces
            .get(hash_start..hash_end)
            .and_then(|h| <[u8; 20]>::try_from(h).ok())
            .ok_or_else(|| io::Error::other("piece hash not found"))?;

        Ok(PieceInfo {
            length: piece_length,
            expected_hash,
        })
    }
    pub fn download_piece_from_peer(
        &self,
        peer: &mut Peer,
        output: &Path,
        piece_index: usize,
    ) -> io::Result<()> {
        let info = self.get_piece_info(piece_index)?;
        let piece = receive_piece(peer, piece_index, info.length)?;
        verify_piece(&piece, &info.expected_hash, piece_index)?;
        File::create(output)?.write_all(&piece)
    }
    pub fn download_single_piece(&self, output: &Path, piece_index: usize) -> io::Result<()> {
        // A bad index fails identically on every peer, so reject it before retrying.
        self.get_piece_info(piece_index)?;

        loop {
            let mut peer = self.find_next_active_peer()?;

            match self.download_piece_from_peer(&mut peer, output, piece_index) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("Peer {} failed piece {}: {}", peer.addr, piece_index, e);
                    thread::sleep(retry_delay());
                }
            }
        }
    }
    pub fn download_all(&self, output: &Path) -> io::Result<()> {
        let piece_count = self.torrent.pieces.len() / 20;
        let next = AtomicUsize::new(0);
        let failure: Mutex<Option<io::Error>> = Mutex::new(None);
        let peer_count = self.peers.lock().unwrap().len();
        let num_workers = MAX_WORKERS.min(piece_count).min(peer_count);
        thread::scope(|s| {
            for _ in 0..num_workers {
                s.spawn(|| {
                    loop {
                        if failure.lock().unwrap().is_some() {
                            break;
                        }
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        if idx >= piece_count {
                            break;
                        }
                        let path = temp_piece_path(output, idx);
                        if let Err(e) = self.download_single_piece(&path, idx) {
                            failure.lock().unwrap().get_or_insert(e);
                            break;
                        }
                    }
                });
            }
        });

        if let Some(e) = failure.into_inner().unwrap() {
            return Err(e);
        }
        assemble_file(output, piece_count)
    }
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut result = String::new();

    for byte in bytes {
        result.push_str(&format!("%{:02X}", byte));
    }

    result
}

fn send_interested(stream: &mut TcpStream) -> io::Result<()> {
    // Length = 1, message ID = 2.
    stream.write_all(&[0, 0, 0, 1, 2])
}

fn send_request(stream: &mut TcpStream, index: u32, begin: u32, length: u32) -> io::Result<()> {
    let mut message = Vec::with_capacity(17);

    // Message length = 13 (ID + payload).
    message.extend_from_slice(&13u32.to_be_bytes());

    // Message ID = 6 (request).
    message.push(6);

    // Payload.
    message.extend_from_slice(&index.to_be_bytes());
    message.extend_from_slice(&begin.to_be_bytes());
    message.extend_from_slice(&length.to_be_bytes());

    stream.write_all(&message)
}

fn assemble_file(output: &Path, piece_count: usize) -> io::Result<()> {
    let mut output_file = File::create(output)?;

    for piece_index in 0..piece_count {
        let path = temp_piece_path(output, piece_index);

        let mut piece_file = File::open(&path)?;
        io::copy(&mut piece_file, &mut output_file)?;

        fs::remove_file(path)?;
    }

    Ok(())
}

fn temp_piece_path(output: &Path, piece_index: usize) -> PathBuf {
    let mut name = output.as_os_str().to_os_string();
    name.push(format!(".{piece_index}.tmp"));

    PathBuf::from(name)
}

fn receive_block(
    peer: &mut Peer,
    index: u32,
    begin: u32,
    expected_len: usize,
) -> io::Result<Vec<u8>> {
    loop {
        if let Message::Piece {
            index: i,
            begin: b,
            block,
        } = Message::read_from(&mut peer.stream)?
        {
            if i != index || b != begin {
                continue;
            }
            if block.len() != expected_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected block length",
                ));
            }
            return Ok(block);
        }
    }
}

fn receive_piece(peer: &mut Peer, piece_index: usize, piece_length: usize) -> io::Result<Vec<u8>> {
    let index = u32::try_from(piece_index)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "piece index too large"))?;

    wait_for(peer, |m| matches!(m, Message::Bitfield { .. }))?;
    send_interested(&mut peer.stream)?;
    wait_for(peer, |m| matches!(m, Message::Unchoke))?;

    let mut piece = vec![0u8; piece_length];
    let mut begin = 0;

    while begin < piece_length {
        let length = BLOCK_SIZE.min(piece_length - begin);

        // `begin` and `length` are bounded by `piece_length`, which fits in u32
        // for any valid torrent; fail loudly instead of truncating.
        let (begin_u32, length_u32) = (
            u32::try_from(begin).map_err(|_| io::Error::other("offset too large"))?,
            u32::try_from(length).map_err(|_| io::Error::other("block too large"))?,
        );

        send_request(&mut peer.stream, index, begin_u32, length_u32)?;
        let block = receive_block(peer, index, begin_u32, length)?;

        piece[begin..begin + length].copy_from_slice(&block);
        begin += length;
    }

    Ok(piece)
}

fn wait_for(peer: &mut Peer, accept: impl Fn(&Message) -> bool) -> io::Result<Message> {
    loop {
        let msg = Message::read_from(&mut peer.stream)?;
        if accept(&msg) {
            return Ok(msg);
        }
    }
}

fn verify_piece(piece: &[u8], expected_hash: &[u8; 20], piece_index: usize) -> io::Result<()> {
    if Sha1::digest(piece).as_slice() != expected_hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("piece {piece_index} hash mismatch"),
        ));
    }
    Ok(())
}

fn io_invalid_data(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn retry_delay() -> Duration {
    RETRY_DELAY_MIN + (RETRY_DELAY_MAX - RETRY_DELAY_MIN).mul_f64(rand::random::<f64>())
}

pub fn handshake(
    peer_addr: SocketAddr,
    info_hash: &[u8; 20],
    client_id: &[u8; 20],
    extensions: Option<&[(&str, u8)]>,
) -> io::Result<Peer> {
    let mut stream = TcpStream::connect(peer_addr)?;

    // Base handshake
    let mut request = [0u8; 68];
    request[0] = 19;
    request[1..20].copy_from_slice(b"BitTorrent protocol");
    request[28..48].copy_from_slice(info_hash);
    request[48..68].copy_from_slice(client_id);

    if extensions.is_some() {
        request[25] = 0x10;
    }

    stream.write_all(&request)?;

    // Receive base handshake
    let mut response = [0u8; 68];
    stream.read_exact(&mut response)?;

    if response[0] != 19 || &response[1..20] != b"BitTorrent protocol" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid handshake response",
        ));
    }

    if response[28..48] != info_hash[..] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "info hash mismatch",
        ));
    }

    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&response[48..68]);

    let peer_supports_extensions = response[25] & 0x10 != 0;

    let mut peer = Peer {
        peer_id,
        addr: peer_addr,
        stream,
        extensions: None,
    };

    if peer_supports_extensions {
        if let Some(extensions) = extensions {
            // Wait for the peer's bitfield.
            wait_for(&mut peer, |message| {
                matches!(message, Message::Bitfield { .. })
            })?;

            // Send our extension handshake.
            send_extension_handshake(&mut peer, extensions)?;

            // Receive the peer's extension handshake.
            let message = wait_for(&mut peer, |message| {
                matches!(
                    message,
                    Message::Extended {
                        extension_id: 0,
                        ..
                    }
                )
            })?;

            let Message::Extended { payload, .. } = message else {
                unreachable!()
            };

            peer.extensions = Some(parse_extension_mapping(&payload)?);
        }
    }

    Ok(peer)
}

fn send_extension_handshake(peer: &mut Peer, extensions: &[(&str, u8)]) -> io::Result<()> {
    let mut payload = Vec::new();

    // Bencode: d1:md...ee
    payload.extend_from_slice(b"d1:md");

    for (name, id) in extensions {
        payload.extend_from_slice(format!("{}:", name.len()).as_bytes());
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(format!("i{}e", id).as_bytes());
    }

    payload.extend_from_slice(b"ee");

    let length = (payload.len() + 2) as u32;

    peer.stream.write_all(&length.to_be_bytes())?;
    peer.stream
        .write_all(&[EXTENSION_MESSAGE_ID, EXTENSION_HANDSHAKE_ID])?;
    peer.stream.write_all(&payload)?;

    Ok(())
}

fn parse_extension_mapping(payload: &[u8]) -> io::Result<HashMap<String, u8>> {
    let (value, _) = decode_bencoded_value(payload);

    let BencodeValue::Dictionary(dictionary) = value else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected extension handshake dictionary",
        ));
    };

    let Some(BencodeValue::Dictionary(mapping)) = dictionary.get(b"m".as_slice()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing extension mapping",
        ));
    };

    let mut extensions = HashMap::new();

    for (name, value) in mapping {
        let BencodeValue::Integer(id) = value else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected extension ID integer",
            ));
        };

        let id = u8::try_from(*id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid extension ID"))?;

        let name = String::from_utf8(name.clone())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid extension name"))?;

        extensions.insert(name, id);
    }

    Ok(extensions)
}

pub fn fetch_peers(
    tracker_url: &str,
    info_hash: &[u8; 20],
    client_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
) -> io::Result<Vec<SocketAddr>> {
    let info_hash = percent_encode(info_hash);
    let client_id = percent_encode(client_id);

    let separator = if tracker_url.contains('?') { '&' } else { '?' };

    let url = format!(
        "{}{}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1",
        tracker_url, separator, info_hash, client_id, port, uploaded, downloaded, left,
    );

    let body = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(io::Error::other)?;

    let (value, _) = decode_bencoded_value(&body);

    let BencodeValue::Dictionary(dictionary) = value else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected tracker response dictionary",
        ));
    };

    let Some(BencodeValue::String(peers)) = dictionary.get(b"peers".as_slice()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected peers string",
        ));
    };

    Ok(peers
        .chunks_exact(6)
        .map(|peer| {
            let ip = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);
            let port = u16::from_be_bytes([peer[4], peer[5]]);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        })
        .collect())
}

pub fn parse_magnet(magnet_link: &str) -> io::Result<(String, Option<String>)> {
    let url = Url::parse(magnet_link).map_err(|_| io_invalid_data("invalid magnet url"))?;

    let info_hash = url
        .query_pairs()
        .find(|(key, _)| key == "xt")
        .map(|(_, value)| value)
        .ok_or(io_invalid_data("xt key not found"))?;

    let info_hash = info_hash
        .strip_prefix("urn:btih:")
        .ok_or(io_invalid_data("invalid xt value"))?
        .to_string();

    let tracker_url = url
        .query_pairs()
        .find(|(key, _)| key == "tr")
        .map(|(_, value)| value.to_string());

    Ok((info_hash, tracker_url))
}

pub fn manget_metdata_peer(magnet: &str) -> io::Result<Peer> {
    let (info_hash, tracker_url) = parse_magnet(magnet)?;

    let info_hash: [u8; 20] = hex::decode(info_hash)
        .map_err(|_| io_invalid_data("info_hash malformed hex format"))?
        .try_into()
        .map_err(|_| io_invalid_data("invalid info_hash length"))?;

    let mut client_id = [0u8; 20];
    rand::rng().fill(&mut client_id);

    let tracker_url = tracker_url.ok_or(io_invalid_data("Missing tracker URL"))?;

    let peers = fetch_peers(
        &tracker_url,
        &info_hash,
        &client_id,
        6881,
        0,
        0,
        1, // Metadata not downloaded yet; actual file length is unknown.
    )?;

    let peer_addr = *peers.first().ok_or(io_invalid_data("No peers available"))?;
    let extensions = [("ut_metadata", 1)];
    handshake(peer_addr, &info_hash, &client_id, Some(&extensions))
}

pub fn request_metadata(peer: &mut Peer) -> io::Result<Vec<u8>> {
    let extensions = peer
        .extensions
        .as_ref()
        .ok_or_else(|| io_invalid_data("peer does not support extensions"))?;

    let extension_id = *extensions
        .get("ut_metadata")
        .ok_or_else(|| io_invalid_data("peer does not support ut_metadata"))?;

    if extension_id == 0 {
        return Err(io::Error::other("ut_metadata is disabled"));
    }

    send_metadata_request(peer, extension_id, 0)?;
    receive_metadata(peer, extension_id, 0)
}
/*
* Length (4 bytes)
    Message ID: 20
    Extension ID: peer's ut_metadata ID
    Bencoded payload:
        msg_type = 0
        piece = 0
*/
fn send_metadata_request(peer: &mut Peer, extension_id: u8, piece: u32) -> io::Result<()> {
    // Bencode: d8:msg_typei0e5:piecei0ee
    let payload = format!("d8:msg_typei{}e5:piecei{}ee", METADATA_REQUEST, piece);

    let length = (2 + payload.len()) as u32;

    peer.stream.write_all(&length.to_be_bytes())?;
    peer.stream
        .write_all(&[EXTENSION_MESSAGE_ID, extension_id])?;
    peer.stream.write_all(payload.as_bytes())?;

    Ok(())
}

fn receive_metadata(peer: &mut Peer, extension_id: u8, piece: u32) -> io::Result<Vec<u8>> {
    let message = wait_for(peer, |message| {
        matches!(
            message,
            Message::Extended {
                extension_id: id,
                ..
            } if *id == extension_id
        )
    })?;

    let Message::Extended { payload, .. } = message else {
        unreachable!()
    };

    let (value, metadata) = decode_bencoded_value(&payload);

    let BencodeValue::Dictionary(header) = value else {
        return Err(io_invalid_data("expected metadata header dictionary"));
    };

    let get_integer = |key: &[u8]| -> io::Result<i64> {
        match header.get(key) {
            Some(BencodeValue::Integer(value)) => Ok(*value),
            _ => Err(io_invalid_data("missing or invalid metadata header field")),
        }
    };

    let msg_type = get_integer(b"msg_type")?;
    let response_piece = get_integer(b"piece")?;

    if msg_type == METADATA_REJECT {
        return Err(io_invalid_data("peer rejected metadata request"));
    }

    if msg_type != METADATA_DATA {
        return Err(io_invalid_data("unexpected metadata message type"));
    }

    if response_piece != i64::from(piece) {
        return Err(io_invalid_data("unexpected metadata piece index"));
    }

    let total_size = usize::try_from(get_integer(b"total_size")?)
        .map_err(|_| io_invalid_data("invalid metadata size"))?;

    if metadata.len() != total_size {
        return Err(io_invalid_data("metadata size mismatch"));
    }

    Ok(metadata.to_vec())
}

pub fn print_magnet_info(magnet: &str) -> io::Result<()> {
    let mut peer = manget_metdata_peer(magnet)?;
    let metadata = request_metadata(&mut peer)?;

    let (value, _) = decode_bencoded_value(&metadata);

    let BencodeValue::Dictionary(info) = value else {
        return Err(io_invalid_data("expected info dictionary"));
    };

    let length = match info.get(b"length".as_slice()) {
        Some(BencodeValue::Integer(length)) => *length,
        _ => return Err(io_invalid_data("missing or invalid length")),
    };

    let piece_length = match info.get(b"piece length".as_slice()) {
        Some(BencodeValue::Integer(length)) => *length,
        _ => return Err(io_invalid_data("missing or invalid piece length")),
    };

    let pieces = match info.get(b"pieces".as_slice()) {
        Some(BencodeValue::String(pieces)) => pieces,
        _ => return Err(io_invalid_data("missing or invalid pieces")),
    };

    let info_hash = Sha1::digest(&metadata);

    println!("Tracker URL: {}", parse_magnet(magnet)?.1.unwrap());
    println!("Length: {length}");
    println!("Info Hash: {}", hex::encode(info_hash));
    println!("Piece Length: {piece_length}");
    println!("Piece Hashes:");

    for piece_hash in pieces.chunks(20) {
        println!("{}", hex::encode(piece_hash));
    }

    Ok(())
}
