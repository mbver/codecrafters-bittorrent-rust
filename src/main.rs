use std::process::ExitCode;
use std::{env, net::SocketAddr, path::Path};

use reqwest::Url;

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
            let peer = client.handshake(peer_addr)?;
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
            let magnet_link = &args[2];
            let url = Url::parse(magnet_link)?;

            let info_hash = url
                .query_pairs()
                .find(|(key, _)| key == "xt")
                .map(|(_, value)| value)
                .unwrap();

            let info_hash = info_hash.strip_prefix("urn:btih:").unwrap();

            println!("Info Hash: {}", info_hash);
            let tracker_url = url
                .query_pairs()
                .find(|(key, _)| key == "tr")
                .map(|(_, value)| value);
            println!("Tracker URL: {}", tracker_url.unwrap());
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
    Ok(())
}
