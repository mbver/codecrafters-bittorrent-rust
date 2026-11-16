use std::{collections::HashMap, env, fmt, fs};

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

fn main() {
    let args: Vec<String> = env::args().collect();

    match args[1].as_str() {
        "decode" => {
            let encoded = args[2].as_bytes();
            let (value, _) = decode_bencoded_value(encoded);

            // Temporary: decode command currently supports
            // displaying simple string and integer values.
            println!("{}", value);
        }
        "info" => {
            let encoded = fs::read(&args[2]).unwrap();
            let (value, _) = decode_bencoded_value(&encoded);

            let BencodeValue::Dictionary(root) = value else {
                panic!("Expected root dictionary");
            };

            let announce = root.get(b"announce".as_slice()).unwrap();

            let info = root.get(b"info".as_slice()).unwrap();

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

            println!("Tracker URL: {}", String::from_utf8_lossy(announce));
            println!("Length: {}", length);
        }
        command => {
            println!("unknown command: {}", command);
        }
    }
}
