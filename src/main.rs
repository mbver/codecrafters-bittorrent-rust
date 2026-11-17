use std::{env, net::SocketAddr};

use crate::{bencode::decode_bencoded_value, client::Client, torrent::Torrent};
mod bencode;
mod client;
mod torrent;

fn main() {
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
            let client = Client::new(torrent);
            let peers = match client.fetch_peers() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return;
                }
            };

            for peer in peers {
                println!("{}", peer);
            }
        }
        "handshake" => {
            let torrent = Torrent::from_file(&args[2]);
            let client = Client::new(torrent);

            let peer_addr: SocketAddr = args[3].parse().unwrap();
            let peer_id = client.handshake(peer_addr).unwrap();
            println!("Peer ID: {}", hex::encode(peer_id));
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
}
