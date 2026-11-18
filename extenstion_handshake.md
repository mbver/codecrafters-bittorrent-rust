```rs
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};

const EXTENSION_MESSAGE_ID: u8 = 20;
const EXTENSION_HANDSHAKE_ID: u8 = 0;
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

pub struct Peer {
    pub peer_id: [u8; 20],
    pub addr: SocketAddr,
    pub stream: TcpStream,
    pub extensions: Option<HashMap<String, u8>>,
}

pub fn handshake(
    peer_addr: SocketAddr,
    info_hash: &[u8; 20],
    client_id: &[u8; 20],
    extensions: Option<&[(&str, u8)]>,
) -> io::Result<Peer> {
    let mut stream = TcpStream::connect(peer_addr)?;

    // Base handshake
    let mut request = [0u8; 68];
    request[0] = 19;
    request[1..20].copy_from_slice(b"BitTorrent protocol");
    request[28..48].copy_from_slice(info_hash);
    request[48..68].copy_from_slice(client_id);

    if extensions.is_some() {
        request[25] = 0x10;
    }

    stream.write_all(&request)?;

    // Receive base handshake
    let mut response = [0u8; 68];
    stream.read_exact(&mut response)?;

    if response[0] != 19 || &response[1..20] != b"BitTorrent protocol" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid handshake response",
        ));
    }

    if response[28..48] != info_hash[..] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "info hash mismatch",
        ));
    }

    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&response[48..68]);

    let peer_supports_extensions = response[25] & 0x10 != 0;

    let mut peer = Peer {
        peer_id,
        addr: peer_addr,
        stream,
        extensions: None,
    };

    if peer_supports_extensions {
        if let Some(extensions) = extensions {
            // Wait for the peer's bitfield.
            wait_for(&mut peer, |message| {
                matches!(message, Message::Bitfield { .. })
            })?;

            // Send our extension handshake.
            send_extension_handshake(&mut peer, extensions)?;

            // Receive the peer's extension handshake.
            let message = wait_for(&mut peer, |message| {
                matches!(
                    message,
                    Message::Extended {
                        extension_id: 0,
                        ..
                    }
                )
            })?;

            let Message::Extended { payload, .. } = message else {
                unreachable!()
            };

            peer.extensions = Some(parse_extension_mapping(&payload)?);
        }
    }

    Ok(peer)
}
```

```rs
fn send_extension_handshake(
    peer: &mut Peer,
    extensions: &[(&str, u8)],
) -> io::Result<()> {
    let mut payload = Vec::new();

    // Bencode: d1:md...ee
    payload.extend_from_slice(b"d1:md");

    for (name, id) in extensions {
        payload.extend_from_slice(format!("{}:", name.len()).as_bytes());
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(format!("i{}e", id).as_bytes());
    }

    payload.extend_from_slice(b"ee");

    let length = (payload.len() + 2) as u32;

    peer.stream.write_all(&length.to_be_bytes())?;
    peer.stream.write_all(&[20, 0])?;
    peer.stream.write_all(&payload)?;

    Ok(())
}
```

```rs
fn receive_extension_handshake(
    stream: &mut TcpStream,
) -> io::Result<HashMap<String, u8>> {
    loop {
        let mut length_buffer = [0u8; 4];
        stream.read_exact(&mut length_buffer)?;

        let length = u32::from_be_bytes(length_buffer) as usize;

        // Keepalive message.
        if length == 0 {
            continue;
        }

        if length > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "peer message too large",
            ));
        }

        let mut message = vec![0u8; length];
        stream.read_exact(&mut message)?;

        // Extension messages need at least two bytes:
        // message ID and extension message ID.
        if message.len() < 2 {
            continue;
        }

        if message[0] != EXTENSION_MESSAGE_ID
            || message[1] != EXTENSION_HANDSHAKE_ID
        {
            // Ignore other messages, such as bitfield.
            continue;
        }

        return parse_extension_mapping(&message[2..]);
    }
}
```

```rs
fn parse_extension_mapping(
    payload: &[u8],
) -> io::Result<HashMap<String, u8>> {
    let (value, _) = decode_bencoded_value(payload);

    let BencodeValue::Dictionary(dictionary) = value else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected extension handshake dictionary",
        ));
    };

    let Some(BencodeValue::Dictionary(mapping)) =
        dictionary.get(b"m".as_slice())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing extension mapping",
        ));
    };

    let mut extensions = HashMap::new();

    for (name, value) in mapping {
        let BencodeValue::Integer(id) = value else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected extension ID integer",
            ));
        };

        let id = u8::try_from(*id).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid extension ID",
            )
        })?;

        let name = String::from_utf8(name.clone()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid extension name",
            )
        })?;

        extensions.insert(name, id);
    }

    Ok(extensions)
}
```

```rs
let extensions = [
    ("ut_metadata", 1),
];

let peer = handshake(
    peer_addr,
    &info_hash,
    &client_id,
    Some(&extensions),
)?;

println!("Peer ID: {}", hex::encode(peer.peer_id));

if let Some(extensions) = &peer.extensions {
    println!("Peer extensions: {extensions:?}");
}
```

```rs
pub fn handshake(
    peer_addr: SocketAddr,
    info_hash: &[u8; 20],
    client_id: &[u8; 20],
    extensions: Option<&[(&str, u8)]>,
) -> io::Result<Peer>
```

```rs
pub struct Peer {
    pub peer_id: [u8; 20],
    pub addr: SocketAddr,
    pub stream: TcpStream,
    pub extensions: Option<HashMap<String, u8>>,
}
```
