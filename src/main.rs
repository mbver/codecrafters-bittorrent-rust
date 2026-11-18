use std::process::ExitCode;
use std::{env, net::SocketAddr, path::Path};

use rand::RngExt;
use reqwest::Url;

use crate::client::{fetch_peers, handshake};
use crate::{bencode::decode_bencoded_value, client::Client, torrent::Torrent};
mod bencode;
mod client;
mod message;
mod torrent;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    match args[1].as_str() {
        "decode" => {
            let encoded = args[2].as_bytes();
            let (value, _) = decode_bencoded_value(encoded);

            // Temporary: decode command currently supports
            // displaying simple string and integer values.
            println!("{}", value.to_json());
        }
        "info" => {
            let torrent = Torrent::from_file(&args[2]);
            println!("Tracker URL: {}", torrent.announce,);
            println!("Length: {}", torrent.length);
            println!("Info Hash: {}", hex::encode(torrent.info_hash));

            println!("Piece Length: {}", torrent.piece_length);
            println!("Piece Hashes:");

            for piece_hash in torrent.pieces.chunks(20) {
                println!("{}", hex::encode(piece_hash))
            }
        }
        "peers" => {
            let torrent = Torrent::from_file(&args[2]);
            let client = Client::new(torrent)?;
            let peers = client.fetch_peers()?;

            for peer in peers {
                println!("{}", peer);
            }
        }
        "handshake" => {
            let torrent = Torrent::from_file(&args[2]);
            let client = Client::new(torrent)?;

            let peer_addr: SocketAddr = args[3].parse()?;
            let peer = client.handshake(peer_addr, None)?;
            println!("Peer ID: {}", hex::encode(peer.peer_id));
        }
        "download_piece" => {
            let output = Path::new(&args[3]);
            let torrent = Torrent::from_file(&args[4]);
            let client = Client::new(torrent)?;

            let piece_index: usize = args[5].parse()?;

            client.download_single_piece(output, piece_index)?;
        }
        "download" => {
            let output = Path::new(&args[3]);
            let torrent = Torrent::from_file(&args[4]);
            let client = Client::new(torrent)?;

            client.download_all(output)?;
        }
        "magnet_parse" => {
            let (info_hash, tracker_url) = parse_magnet(&args[2])?;

            println!("Info Hash: {}", info_hash);

            if let Some(tracker_url) = tracker_url {
                println!("Tracker URL: {}", tracker_url);
            }
        }
        "magnet_handshake" => {
            let (info_hash, tracker_url) = parse_magnet(&args[2])?;

            let info_hash: [u8; 20] = hex::decode(info_hash)?
                .try_into()
                .map_err(|_| "Invalid info hash length")?;

            let mut client_id = [0u8; 20];
            rand::rng().fill(&mut client_id);

            let tracker_url = tracker_url.ok_or("Missing tracker URL")?;

            let peers = fetch_peers(
                &tracker_url,
                &info_hash,
                &client_id,
                6881,
                0,
                0,
                1, // Metadata not downloaded yet; actual file length is unknown.
            )?;

            let peer_addr = *peers.first().ok_or("No peers available")?;
            let extensions = [("ut_metadata", 1)];

            let peer = handshake(peer_addr, &info_hash, &client_id, Some(&extensions))?;
            println!("Peer ID: {}", hex::encode(peer.peer_id));
            let Some(extensions) = &peer.extensions else {
                return Ok(());
            };
            if let Some(extension_id) = extensions.get("ut_metadata") {
                println!("Peer Metadata Extension ID: {}", extension_id)
            }
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
    Ok(())
}

fn parse_magnet(magnet_link: &str) -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
    let url = Url::parse(magnet_link)?;

    let info_hash = url
        .query_pairs()
        .find(|(key, _)| key == "xt")
        .map(|(_, value)| value)
        .ok_or("Missing xt parameter")?;

    let info_hash = info_hash
        .strip_prefix("urn:btih:")
        .ok_or("Invalid xt parameter")?
        .to_string();

    let tracker_url = url
        .query_pairs()
        .find(|(key, _)| key == "tr")
        .map(|(_, value)| value.to_string());

    Ok((info_hash, tracker_url))
}
