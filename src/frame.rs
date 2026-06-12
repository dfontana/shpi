//! Wire frame: a host label plus an opaque message body, encoded with bincode.
//!
//! `send` writes exactly one frame per TCP connection then closes, so the daemon
//! reads the connection to EOF to get one frame and forwards the bytes verbatim
//! onto the output FIFO, where `watch` decodes a continuous stream of them.

use std::io::Read;

/// Upper bound on a single decoded frame, so a corrupt or hostile stream can't
/// drive an unbounded allocation.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, bincode::Encode, bincode::Decode)]
pub struct Frame {
    pub host: String,
    pub body: Vec<u8>,
}

fn config() -> impl bincode::config::Config {
    bincode::config::standard().with_limit::<MAX_FRAME_LEN>()
}

/// Encode one frame to its wire bytes.
pub fn encode_frame(host: &str, body: &[u8]) -> Vec<u8> {
    let frame = Frame {
        host: host.to_string(),
        body: body.to_vec(),
    };
    bincode::encode_to_vec(&frame, config()).expect("frame encode is infallible")
}

/// Read one frame from a blocking reader. `Ok(None)` at EOF (no further frame);
/// `Err` on a malformed frame.
pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<Option<Frame>> {
    use bincode::error::DecodeError;
    match bincode::decode_from_std_read(r, config()) {
        Ok(frame) => Ok(Some(frame)),
        // Reader exhausted at or partway through a frame — treat as end of stream.
        Err(DecodeError::UnexpectedEnd { .. }) => Ok(None),
        Err(DecodeError::Io { inner, .. }) if inner.kind() == std::io::ErrorKind::UnexpectedEof => {
            Ok(None)
        }
        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    }
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
}
