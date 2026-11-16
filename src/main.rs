use std::env;

// Available if you need it!
// use serde_bencode

fn decode_bencoded_value(input: &str) -> (serde_json::Value, &str) {
    match input.chars().next().unwrap() {
        '0'..='9' => {
            let (length, rest) = input.split_once(':').unwrap();
            let length = length.parse::<usize>().unwrap();

            let (string, remaining) = rest.split_at(length);

            (serde_json::Value::String(string.to_string()), remaining)
        }
        'i' => {
            let end = input.find('e').unwrap();
            let number = input[1..end].parse::<i64>().unwrap();

            (serde_json::Value::Number(number.into()), &input[end + 1..])
        }
        'l' => {
            let mut remaining = &input[1..];
            let mut values = Vec::new();

            while !remaining.starts_with('e') {
                let (value, rest) = decode_bencoded_value(remaining);
                values.push(value);
                remaining = rest;
            }

            (serde_json::Value::Array(values), &remaining[1..])
        }
        _ => panic!("Unhandled encoded value: {}", input),
    }
}

// Usage: your_program.sh decode "<encoded_value>"
fn main() {
    let args: Vec<String> = env::args().collect();
    let command = &args[1];

    if command == "decode" {
        // You can use print statements as follows for debugging, they'll be visible when running tests.
        eprintln!("Logs from your program will appear here!");

        // TODO: Uncomment the code below to pass the first stage
        let encoded_value = &args[2];
        let (decoded_value, _) = decode_bencoded_value(encoded_value);
        println!("{}", decoded_value.to_string());
    } else {
        println!("unknown command: {}", args[1])
    }
}
