use std::process::ExitCode;
use std::{env, net::SocketAddr, path::Path};

use crate::client::{manget_metdata_peer, parse_magnet, print_torrent, torrent_from_magnet};
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
            let magnet = &args[2];
            let peer = manget_metdata_peer(magnet)?;
            println!("Peer ID: {}", hex::encode(peer.peer_id));
            let Some(extensions) = &peer.extensions else {
                return Ok(());
            };
            if let Some(extension_id) = extensions.get("ut_metadata") {
                println!("Peer Metadata Extension ID: {}", extension_id)
            }
        }
        "magnet_info" => {
            let torrent = torrent_from_magnet(&args[2])?;
            print_torrent(&torrent);
        }
        "magnet_download_piece" => {
            let output = Path::new(&args[3]);
            let magnet = &args[4];
            let piece_index = args[5].parse()?;

            let torrent = torrent_from_magnet(magnet)?;
            let client = Client::new(torrent)?;
            client.download_single_piece(output, piece_index)?;
        }
        "magnet_download" => {
            let output = Path::new(&args[3]);
            let magnet = &args[4];
            let torrent = torrent_from_magnet(magnet)?;
            let client = Client::new(torrent)?;

            client.download_all(output)?;
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
    Ok(())
}
