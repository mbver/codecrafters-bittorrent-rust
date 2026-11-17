use std::{
    error::Error,
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::Path,
};

use rand::RngExt;
use sha1::{Digest, Sha1};

use crate::{
    bencode::{BencodeValue, decode_bencoded_value},
    message::Message,
    torrent::Torrent,
};

pub struct Client {
    torrent: Torrent,
    peer_id: [u8; 20],
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

impl Client {
    pub fn new(torrent: Torrent) -> Self {
        let mut peer_id = [0u8; 20];
        rand::rng().fill(&mut peer_id);

        Self {
            left: torrent.length,
            torrent,
            peer_id,
            port: 6881,
            uploaded: 0,
            downloaded: 0,
        }
    }
    pub fn fetch_peers(&self) -> Result<Vec<SocketAddr>, Box<dyn Error>> {
        let info_hash = percent_encode(&self.torrent.info_hash);
        let peer_id = percent_encode(&self.peer_id);

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
            peer_id,
            self.port,
            self.uploaded,
            self.downloaded,
            self.left,
        );

        let response = reqwest::blocking::get(url)?.error_for_status()?;
        let body = response.bytes()?;

        let (value, _) = decode_bencoded_value(&body);

        let BencodeValue::Dictionary(dictionary) = value else {
            return Err("Expected tracker response dictionary".into());
        };

        let Some(BencodeValue::String(peers)) = dictionary.get(b"peers".as_slice()) else {
            return Err("Expected peers string".into());
        };

        let mut result = Vec::new();

        // Compact IPv4 peer format:
        // 4 bytes IP + 2 bytes port.
        for peer in peers.chunks_exact(6) {
            let ip = Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3]);

            let port = u16::from_be_bytes([peer[4], peer[5]]);

            result.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
        }

        Ok(result)
    }
    pub fn handshake(&self, peer_addr: SocketAddr) -> Result<Peer, Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(peer_addr)?;

        // Build the 68-byte handshake.
        let mut request = [0u8; 68];

        request[0] = 19;
        request[1..20].copy_from_slice(b"BitTorrent protocol");
        request[28..48].copy_from_slice(&self.torrent.info_hash);
        request[48..68].copy_from_slice(&self.peer_id);

        stream.write_all(&request)?;

        // Receive the handshake response.
        let mut response = [0u8; 68];
        stream.read_exact(&mut response)?;

        // Validate the response.
        if response[0] != 19 || &response[1..20] != b"BitTorrent protocol" {
            return Err("Invalid handshake response".into());
        }

        if response[28..48] != self.torrent.info_hash {
            return Err("Info hash mismatch".into());
        }

        let mut peer_id = [0u8; 20];
        peer_id.copy_from_slice(&response[48..68]);

        Ok(Peer {
            peer_id,
            addr: peer_addr,
            stream,
        })
    }
    pub fn download_piece(&self, output: &Path, piece_index: usize) -> io::Result<()> {
        const BLOCK_SIZE: usize = 16 * 1024;

        // Calculate the requested piece length.
        let piece_length = usize::try_from(self.torrent.piece_length)
            .map_err(|_| io::Error::other("piece length too large"))?;

        let total_length = usize::try_from(self.torrent.length)
            .map_err(|_| io::Error::other("torrent length too large"))?;

        let piece_offset = piece_index
            .checked_mul(piece_length)
            .ok_or_else(|| io::Error::other("piece offset overflow"))?;

        if piece_length == 0 || piece_offset >= total_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid piece index",
            ));
        }

        let actual_piece_length = piece_length.min(total_length - piece_offset);

        // Get the expected SHA-1 hash.
        let hash_offset = piece_index
            .checked_mul(20)
            .ok_or_else(|| io::Error::other("hash offset overflow"))?;

        let expected_hash = self
            .torrent
            .pieces
            .get(hash_offset..hash_offset + 20)
            .ok_or_else(|| io::Error::other("piece hash not found"))?;

        // Discover peers.
        let peers = self
            .fetch_peers()
            .map_err(|e| io::Error::other(e.to_string()))?;

        if peers.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "tracker returned no peers",
            ));
        }

        // Try peers until one successfully provides the requested piece.
        let mut last_error = None;

        for peer_addr in peers {
            let result = (|| {
                let mut peer = self
                    .handshake(peer_addr)
                    .map_err(|e| io::Error::other(e.to_string()))?;

                self.download_piece_from_peer(
                    &mut peer,
                    piece_index,
                    actual_piece_length,
                    BLOCK_SIZE,
                )
            })();

            match result {
                Ok(piece) => {
                    // Verify SHA-1.
                    let actual_hash = Sha1::digest(&piece);

                    if actual_hash.as_slice() != expected_hash {
                        last_error = Some(io::Error::other(format!(
                            "piece {} hash mismatch",
                            piece_index
                        )));
                        continue;
                    }

                    // Write the downloaded piece.
                    let mut file = File::create(output)?;
                    file.write_all(&piece)?;

                    return Ok(());
                }

                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no usable peers")))
    }
    fn download_piece_from_peer(
        &self,
        peer: &mut Peer,
        piece_index: usize,
        piece_length: usize,
        block_size: usize,
    ) -> io::Result<Vec<u8>> {
        // Wait for bitfield.
        loop {
            match Message::read_from(&mut peer.stream)? {
                Message::Bitfield { .. } => break,
                Message::KeepAlive => continue,
                _ => continue,
            }
        }
        // Tell the peer we want to download.
        send_interested(&mut peer.stream)?;

        // Wait until the peer unchokes us.
        loop {
            match Message::read_from(&mut peer.stream)? {
                Message::Unchoke => break,
                Message::KeepAlive => continue,
                Message::Choke => continue,
                _ => continue,
            }
        }

        let mut piece = vec![0u8; piece_length];

        // Download the piece block by block.
        let mut begin = 0;

        while begin < piece_length {
            let length = block_size.min(piece_length - begin);

            send_request(
                &mut peer.stream,
                piece_index as u32,
                begin as u32,
                length as u32,
            )?;

            // Wait for the requested block.
            loop {
                match Message::read_from(&mut peer.stream)? {
                    Message::Piece {
                        index,
                        begin: received_begin,
                        block,
                    } => {
                        if index != piece_index as u32 || received_begin != begin as u32 {
                            continue;
                        }

                        if block.len() != length {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "unexpected block length",
                            ));
                        }

                        piece[begin..begin + length].copy_from_slice(&block);
                        break;
                    }

                    Message::KeepAlive => continue,

                    _ => continue,
                }
            }

            begin += length;
        }

        Ok(piece)
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
