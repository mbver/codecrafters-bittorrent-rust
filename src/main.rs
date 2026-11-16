use std::env;

// Available if you need it!
// use serde_bencode

fn decode_bencoded_value(encoded_value: &str) -> serde_json::Value {
    match encoded_value.chars().next().unwrap() {
        '0'..='9' => {
            // String: 5:hello
            let (length, rest) = encoded_value.split_once(':').unwrap();
            let length = length.parse::<usize>().unwrap();

            let string = &rest[..length];
            serde_json::Value::String(string.to_string())
        }
        'i' => {
            // Integer: i52e or i-52e
            let number = encoded_value
                .strip_prefix('i')
                .unwrap()
                .strip_suffix('e')
                .unwrap()
                .parse::<i64>()
                .unwrap();

            serde_json::Value::Number(number.into())
        }
        _ => panic!("Unhandled encoded value: {}", encoded_value),
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
        let decoded_value = decode_bencoded_value(encoded_value);
        println!("{}", decoded_value.to_string());
    } else {
        println!("unknown command: {}", args[1])
    }
}
