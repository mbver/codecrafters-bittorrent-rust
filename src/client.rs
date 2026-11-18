use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use std::{
    error::Error,
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::Path,
    sync::Arc,
};
use std::{fs, thread};

const BLOCK_SIZE: usize = 16 * 1024;
const MAX_WORKERS: usize = 8;
const RETRY_DELAY_MIN: Duration = Duration::from_millis(500);
const RETRY_DELAY_MAX: Duration = Duration::from_millis(1500);

use rand::RngExt;
use sha1::{Digest, Sha1};

use crate::{
    bencode::{BencodeValue, decode_bencoded_value},
    message::Message,
    torrent::Torrent,
    workqueue::WorkQueue,
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
        let info_hash = percent_encode(&self.torrent.info_hash);
        let client_id = percent_encode(&self.client_id);

        let separator = if self.torrent.announce.contains('?') {
            '&'
        } else {
            '?'
        };

        let url = format!(
            "{}{}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1",
            self.torrent.announce,
            separator,
            info_hash,
            client_id,
            self.port,
            self.uploaded,
            self.downloaded,
            self.left,
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

        // Compact IPv4 peer format: 4 bytes IP + 2 bytes port.
        Ok(peers
            .chunks_exact(6)
            .map(|peer| {
                let ip = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);
                let port = u16::from_be_bytes([peer[4], peer[5]]);
                SocketAddr::V4(SocketAddrV4::new(ip, port))
            })
            .collect())
    }

    pub fn handshake(&self, peer_addr: SocketAddr) -> io::Result<Peer> {
        let mut stream = TcpStream::connect(peer_addr)?;

        // Build the 68-byte handshake.
        let mut request = [0u8; 68];
        request[0] = 19;
        request[1..20].copy_from_slice(b"BitTorrent protocol");
        request[28..48].copy_from_slice(&self.torrent.info_hash);
        request[48..68].copy_from_slice(&self.client_id);

        stream.write_all(&request)?;

        // Receive and validate the response.
        let mut response = [0u8; 68];
        stream.read_exact(&mut response)?;

        if response[0] != 19 || &response[1..20] != b"BitTorrent protocol" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid handshake response",
            ));
        }

        if response[28..48] != self.torrent.info_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "info hash mismatch",
            ));
        }

        let mut peer_id = [0u8; 20];
        peer_id.copy_from_slice(&response[48..68]);

        Ok(Peer {
            peer_id,
            addr: peer_addr,
            stream,
        })
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

            match self.handshake(addr) {
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

        thread::scope(|s| {
            for _ in 0..MAX_WORKERS.min(piece_count) {
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

fn write_temp_piece(output: &Path, piece_index: usize, piece: &[u8]) -> io::Result<()> {
    let path = temp_piece_path(output, piece_index);

    let mut file = File::create(path)?;
    file.write_all(piece)?;

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
