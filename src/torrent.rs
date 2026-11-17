use std::fs;

use sha1::{Digest, Sha1};

use crate::bencode::{BencodeValue, decode_bencoded_value, encode_bencoded_value};

pub struct Torrent {
    pub announce: String,
    pub info_hash: [u8; 20],
    pub length: u64,
    pub piece_length: u64,
    pub pieces: Vec<u8>,
}

impl Torrent {
    pub fn from_file(path: &str) -> Self {
        let encoded = fs::read(path).unwrap();

        let (value, _) = decode_bencoded_value(&encoded);

        let BencodeValue::Dictionary(root) = value else {
            panic!("Expected root dictionary");
        };

        let announce = match root.get(b"announce".as_slice()).unwrap() {
            BencodeValue::String(bytes) => String::from_utf8(bytes.clone()).unwrap(),
            _ => panic!("Expected announce string"),
        };
        let info = root.get(b"info".as_slice()).unwrap();
        let mut encoded_info = Vec::new();

        encode_bencoded_value(&info, &mut encoded_info);

        let info = match info {
            BencodeValue::Dictionary(info) => info,
            _ => panic!("Expected info dictionary"),
        };

        // Calculate info hash.
        let digest = Sha1::digest(&encoded_info);

        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(&digest);

        // Extract file length.
        let length = match info.get(b"length".as_slice()).unwrap() {
            BencodeValue::Integer(length) => u64::try_from(*length).unwrap(),
            _ => panic!("Expected length integer"),
        };

        // Extract piece length.
        let piece_length = match info.get(b"piece length".as_slice()).unwrap() {
            BencodeValue::Integer(length) => u64::try_from(*length).unwrap(),
            _ => panic!("Expected piece length integer"),
        };

        // Extract piece hashes.
        let pieces = match info.get(b"pieces".as_slice()).unwrap() {
            BencodeValue::String(bytes) => bytes.clone(),
            _ => panic!("Expected pieces string"),
        };

        Self {
            announce,
            info_hash,
            length,
            piece_length,
            pieces,
        }
    }
}
