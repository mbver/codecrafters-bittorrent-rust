use std::io::{self, Read};
use std::net::TcpStream;

pub enum Message {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        piece_index: u32,
    },
    Bitfield {
        bitfield: Vec<u8>,
    },
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    KeepAlive,
    Extended {
        extension_id: u8,
        payload: Vec<u8>,
    },
}

impl Message {
    pub fn read_from(stream: &mut TcpStream) -> io::Result<Self> {
        let mut length_buf = [0u8; 4];
        stream.read_exact(&mut length_buf)?;

        let length = u32::from_be_bytes(length_buf) as usize;

        if length == 0 {
            return Ok(Message::KeepAlive);
        }

        let mut buf = vec![0u8; length];
        stream.read_exact(&mut buf)?;

        let message_id = buf[0];
        let payload = &buf[1..];

        match message_id {
            0 if payload.is_empty() => Ok(Message::Choke),
            1 if payload.is_empty() => Ok(Message::Unchoke),
            2 if payload.is_empty() => Ok(Message::Interested),
            3 if payload.is_empty() => Ok(Message::NotInterested),

            4 if payload.len() == 4 => Ok(Message::Have {
                piece_index: read_u32(payload)?,
            }),

            5 => Ok(Message::Bitfield {
                bitfield: payload.to_vec(),
            }),

            6 if payload.len() == 12 => Ok(Message::Request {
                index: read_u32(&payload[0..4])?,
                begin: read_u32(&payload[4..8])?,
                length: read_u32(&payload[8..12])?,
            }),

            7 if payload.len() >= 8 => Ok(Message::Piece {
                index: read_u32(&payload[0..4])?,
                begin: read_u32(&payload[4..8])?,
                block: payload[8..].to_vec(),
            }),

            8 if payload.len() == 12 => Ok(Message::Cancel {
                index: read_u32(&payload[0..4])?,
                begin: read_u32(&payload[4..8])?,
                length: read_u32(&payload[8..12])?,
            }),
            20 if !payload.is_empty() => Ok(Message::Extended {
                extension_id: payload[0],
                payload: payload[1..].to_vec(),
            }),
            _ => Err(invalid_message_data()),
        }
    }
}

fn invalid_message_data() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid message payload")
}

fn read_u32(bytes: &[u8]) -> io::Result<u32> {
    let bytes: [u8; 4] = bytes.try_into().map_err(|_| invalid_message_data())?;

    Ok(u32::from_be_bytes(bytes))
}
