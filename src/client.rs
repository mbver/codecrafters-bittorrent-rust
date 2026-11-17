use std::{
    error::Error,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
};

use rand::RngExt;

use crate::{
    bencode::{BencodeValue, decode_bencoded_value},
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
    pub fn handshake(&self, peer_addr: SocketAddr) -> Result<[u8; 20], Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(peer_addr)?;

        // Build the 68-byte handshake.
        let mut request = [0u8; 68];

        request[0] = 19;
        request[1..20].copy_from_slice(b"BitTorrent protocol");
        // request[20..28] are already zero.
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

        Ok(peer_id)
    }
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut result = String::new();

    for byte in bytes {
        result.push_str(&format!("%{:02X}", byte));
    }

    result
}
