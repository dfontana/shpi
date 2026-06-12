//! Wire frame codec.
//!
//! Layout: `[frame_len: u32 BE][host_len: u16 BE][host bytes][body bytes]`.
//! `frame_len` counts everything after itself, i.e. `2 + host_len + body_len`.
//! The explicit length prefix delimits frames on a continuous stream (the FIFO).

use std::io::{self, Read};

pub const LEN_PREFIX: usize = 4;

/// Upper bound on a single frame's payload, to reject a garbage length prefix before
/// allocating for it. Generous relative to the string messages shpi carries.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub host: String,
    pub body: Vec<u8>,
}

/// Encode a complete frame (including its length prefix) ready to write to a stream.
pub fn encode_frame(host: &str, body: &[u8]) -> Vec<u8> {
    let host = host.as_bytes();
    let host_len = host.len() as u16;
    let payload_len = (2 + host.len() + body.len()) as u32;
    let mut out = Vec::with_capacity(LEN_PREFIX + payload_len as usize);
    out.extend_from_slice(&payload_len.to_be_bytes());
    out.extend_from_slice(&host_len.to_be_bytes());
    out.extend_from_slice(host);
    out.extend_from_slice(body);
    out
}

/// Parse the payload that follows the `u32` length prefix: `[host_len: u16][host][body]`.
pub fn parse_payload(payload: &[u8]) -> io::Result<Frame> {
    if payload.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too short",
        ));
    }
    let host_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if 2 + host_len > payload.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "host length exceeds frame",
        ));
    }
    let host = String::from_utf8_lossy(&payload[2..2 + host_len]).into_owned();
    let body = payload[2 + host_len..].to_vec();
    Ok(Frame { host, body })
}

/// Read one frame from a blocking reader. Returns `Ok(None)` at a clean EOF (no bytes
/// available before the next frame); errors if EOF interrupts a partial frame.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut len_buf = [0u8; LEN_PREFIX];
    if !read_full_or_eof(r, &mut len_buf)? {
        return Ok(None);
    }
    let payload_len = u32::from_be_bytes(len_buf) as usize;
    if payload_len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length exceeds maximum",
        ));
    }
    let mut payload = vec![0u8; payload_len];
    r.read_exact(&mut payload)?;
    Ok(Some(parse_payload(&payload)?))
}

/// Fill `buf` fully. `Ok(false)` if EOF occurs before any byte is read; `Ok(true)` if
/// filled; `Err(UnexpectedEof)` on a partial read cut short by EOF.
fn read_full_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => {
                if filled == 0 {
                    return Ok(false);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial frame length prefix",
                ));
            }
            n => filled += n,
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_multiple_frames_then_clean_eof() {
        let mut stream = Vec::new();
        stream.extend(encode_frame("host-a", b"hello"));
        stream.extend(encode_frame("host-b", b"multi\nline\tbody"));
        stream.extend(encode_frame("", b"")); // empty host and empty body

        let mut cur = Cursor::new(stream);
        assert_eq!(
            read_frame(&mut cur).unwrap().unwrap(),
            Frame {
                host: "host-a".into(),
                body: b"hello".to_vec()
            }
        );
        assert_eq!(
            read_frame(&mut cur).unwrap().unwrap(),
            Frame {
                host: "host-b".into(),
                body: b"multi\nline\tbody".to_vec()
            }
        );
        assert_eq!(
            read_frame(&mut cur).unwrap().unwrap(),
            Frame {
                host: String::new(),
                body: Vec::new()
            }
        );
        assert!(read_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn truncated_body_is_an_error() {
        let mut bytes = encode_frame("host", b"payload");
        bytes.truncate(bytes.len() - 3);
        assert!(read_frame(&mut Cursor::new(bytes)).is_err());
    }
}
