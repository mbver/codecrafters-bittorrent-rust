use std::{collections::HashMap, env, fmt, fs};

use sha1::{Digest, Sha1};

enum BencodeValue {
    String(Vec<u8>),
    Integer(i64),
    List(Vec<BencodeValue>),
    Dictionary(HashMap<Vec<u8>, BencodeValue>),
}

impl fmt::Display for BencodeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BencodeValue::String(bytes) => {
                write!(f, "{}", String::from_utf8_lossy(bytes))
            }
            BencodeValue::Integer(number) => {
                write!(f, "{}", number)
            }
            BencodeValue::List(values) => {
                write!(f, "[")?;

                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }

                    write!(f, "{}", value)?;
                }

                write!(f, "]")
            }
            BencodeValue::Dictionary(map) => {
                write!(f, "{{")?;

                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }

                    write!(f, "{}: {}", String::from_utf8_lossy(key), value)?;
                }

                write!(f, "}}")
            }
        }
    }
}

impl BencodeValue {
    fn to_json(&self) -> serde_json::Value {
        match self {
            BencodeValue::String(bytes) => {
                serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned())
            }
            BencodeValue::Integer(number) => {
                serde_json::json!(number)
            }
            BencodeValue::List(values) => {
                serde_json::Value::Array(values.iter().map(|v| v.to_json()).collect())
            }
            BencodeValue::Dictionary(map) => {
                let mut object = serde_json::Map::new();

                for (key, value) in map {
                    let key = String::from_utf8_lossy(key).into_owned();
                    object.insert(key, value.to_json());
                }

                serde_json::Value::Object(object)
            }
        }
    }
}

fn decode_bencoded_value(input: &[u8]) -> (BencodeValue, &[u8]) {
    match input[0] {
        b'0'..=b'9' => {
            let colon_index = input.iter().position(|&b| b == b':').unwrap();

            let length = std::str::from_utf8(&input[..colon_index])
                .unwrap()
                .parse::<usize>()
                .unwrap();

            let rest = &input[colon_index + 1..];
            let (string, remaining) = rest.split_at(length);

            (BencodeValue::String(string.to_vec()), remaining)
        }
        b'i' => {
            let end = input.iter().position(|&b| b == b'e').unwrap();

            let number = std::str::from_utf8(&input[1..end])
                .unwrap()
                .parse::<i64>()
                .unwrap();

            (BencodeValue::Integer(number), &input[end + 1..])
        }
        b'l' => {
            let mut remaining = &input[1..];
            let mut values = Vec::new();

            while !remaining.starts_with(b"e") {
                let (value, rest) = decode_bencoded_value(remaining);
                values.push(value);
                remaining = rest;
            }

            (BencodeValue::List(values), &remaining[1..])
        }
        b'd' => {
            let mut remaining = &input[1..];
            let mut map = HashMap::new();

            while !remaining.starts_with(b"e") {
                let (key, rest) = decode_bencoded_value(remaining);
                let (value, rest) = decode_bencoded_value(rest);

                let BencodeValue::String(key) = key else {
                    panic!("Dictionary key must be a string");
                };

                map.insert(key, value);
                remaining = rest;
            }

            (BencodeValue::Dictionary(map), &remaining[1..])
        }
        _ => panic!("Unhandled encoded value: {:?}", input),
    }
}

fn encode_bencoded_value(value: &BencodeValue, output: &mut Vec<u8>) {
    match value {
        BencodeValue::String(bytes) => {
            output.extend_from_slice(bytes.len().to_string().as_bytes());
            output.push(b':');
            output.extend_from_slice(bytes);
        }
        BencodeValue::Integer(number) => {
            output.push(b'i');
            output.extend_from_slice(number.to_string().as_bytes());
            output.push(b'e');
        }
        BencodeValue::List(values) => {
            output.push(b'l');

            for value in values {
                encode_bencoded_value(value, output);
            }

            output.push(b'e');
        }
        BencodeValue::Dictionary(map) => {
            output.push(b'd');

            let mut keys: Vec<&Vec<u8>> = map.keys().collect();
            keys.sort();

            for key in keys {
                encode_bencoded_value(&BencodeValue::String(key.clone()), output);

                encode_bencoded_value(&map[key], output);
            }

            output.push(b'e');
        }
    }
}

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
            let encoded = fs::read(&args[2]).unwrap();
            let (value, _) = decode_bencoded_value(&encoded);

            let BencodeValue::Dictionary(root) = value else {
                panic!("Expected root dictionary");
            };

            let announce = root.get(b"announce".as_slice()).unwrap();

            let info = root.get(b"info".as_slice()).unwrap();
            let mut encoded_info = Vec::new();
            encode_bencoded_value(&info, &mut encoded_info);

            let BencodeValue::String(announce) = announce else {
                panic!("Expected announce string");
            };

            let BencodeValue::Dictionary(info) = info else {
                panic!("Expected info dictionary");
            };

            let length = info.get(b"length".as_slice()).unwrap();

            let BencodeValue::Integer(length) = length else {
                panic!("Expected length integer");
            };
            let hash = Sha1::digest(&encoded_info);
            println!("Tracker URL: {}", String::from_utf8_lossy(announce));
            println!("Length: {}", length);
            println!("Info Hash: {:x}", hash);
            let piece_length = info.get(b"piece length".as_slice()).unwrap();

            let BencodeValue::Integer(piece_length) = piece_length else {
                panic!("Expected piece length integer");
            };

            let pieces = info.get(b"pieces".as_slice()).unwrap();

            let BencodeValue::String(pieces) = pieces else {
                panic!("Expected pieces string");
            };
            println!("Piece Length: {}", piece_length);
            println!("Piece Hashes:");

            for piece_hash in pieces.chunks(20) {
                for byte in piece_hash {
                    print!("{:02x}", byte);
                }

                println!();
            }
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
}
