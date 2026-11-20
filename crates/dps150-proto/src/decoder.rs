use crate::{checksum, cmd, ty, Frame, HEADER_RX};

/// Reject headers whose command, type and length cannot belong together, so a stray
/// header byte with a large length does not stall decoding while we wait for bytes.
fn plausible(command: u8, data_type: u8, len: usize) -> bool {
    if !matches!(
        command,
        cmd::GET | cmd::SET | cmd::BAUD | cmd::SESSION | 0xC0
    ) {
        return false;
    }
    match data_type {
        0 => len == 1,
        ty::OUTPUT => len == 12,
        ty::MODEL | ty::HW_VERSION | ty::FW_VERSION => len <= 64,
        ty::BRIGHTNESS
        | 215
        | ty::METERING
        | ty::OUTPUT_ENABLE
        | ty::PROT_STATE
        | ty::MODE
        | ty::ADDRESS => len == 1,
        // A GET request carries one dummy byte; values are f32.
        192..=227 => len == 4 || len == 1,
        ty::ALL => len >= crate::FULL_STATE_LEN || len == 1,
        _ => false,
    }
}

/// Streaming frame decoder.
///
/// Feed it whatever bytes arrive from the port; it returns complete frames and keeps
/// partial ones for the next call. Bytes that are not a valid frame start are skipped,
/// and a frame with a bad checksum makes the decoder slide forward by one byte so it can
/// resynchronise on the next header.
#[derive(Debug, Clone)]
pub struct FrameDecoder {
    header: u8,
    buf: Vec<u8>,
    /// Number of bytes discarded while resynchronising.
    pub skipped: u64,
    /// Number of frames rejected for a bad checksum.
    pub bad_checksums: u64,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    /// Decoder for device → host frames.
    pub fn new() -> Self {
        Self::with_header(HEADER_RX)
    }

    /// Decoder that accepts frames starting with `header` (the simulator decodes host frames).
    pub fn with_header(header: u8) -> Self {
        Self {
            header,
            buf: Vec::with_capacity(512),
            skipped: 0,
            bad_checksums: 0,
        }
    }

    /// Bytes waiting for the rest of a frame.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Append bytes and return every complete frame found.
    pub fn push(&mut self, data: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(data);
        let mut frames = Vec::new();
        let mut start = 0;
        loop {
            // Find the next header.
            match self.buf[start..].iter().position(|b| *b == self.header) {
                Some(off) => {
                    self.skipped += off as u64;
                    start += off;
                }
                None => {
                    self.skipped += (self.buf.len() - start) as u64;
                    start = self.buf.len();
                    break;
                }
            }
            let rest = &self.buf[start..];
            if rest.len() < 5 {
                break;
            }
            if rest.len() < 4 {
                break;
            }
            let len = rest[3] as usize;
            if !plausible(rest[1], rest[2], len) {
                self.skipped += 1;
                start += 1;
                continue;
            }
            let total = 5 + len;
            if rest.len() < total {
                break;
            }
            let data_type = rest[2];
            let payload = &rest[4..4 + len];
            if checksum(data_type, payload) == rest[4 + len] {
                frames.push(Frame {
                    header: rest[0],
                    command: rest[1],
                    data_type,
                    payload: payload.to_vec(),
                });
                start += total;
            } else {
                self.bad_checksums += 1;
                self.skipped += 1;
                start += 1;
            }
        }
        self.buf.drain(..start);
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_with_header, ty};

    fn rx(ty: u8, payload: &[u8]) -> Vec<u8> {
        encode_with_header(HEADER_RX, crate::cmd::GET, ty, payload)
    }

    #[test]
    fn decodes_split_frames_at_every_offset() {
        let mut stream = rx(ty::OUTPUT, &[1; 12]);
        stream.extend(rx(ty::TEMPERATURE, &25.5f32.to_le_bytes()));
        stream.extend(rx(ty::MODEL, b"DPS-150"));
        for cut in 0..=stream.len() {
            let mut d = FrameDecoder::new();
            let mut frames = d.push(&stream[..cut]);
            frames.extend(d.push(&stream[cut..]));
            assert_eq!(frames.len(), 3, "cut at {cut}");
            assert_eq!(frames[2].payload, b"DPS-150");
            assert_eq!(d.pending(), 0);
        }
    }

    #[test]
    fn byte_by_byte() {
        let stream = rx(ty::OUTPUT_ENABLE, &[1]);
        let mut d = FrameDecoder::new();
        let mut n = 0;
        for b in &stream {
            n += d.push(&[*b]).len();
        }
        assert_eq!(n, 1);
    }

    #[test]
    fn resyncs_after_garbage_and_bad_checksum() {
        let mut stream = vec![0x00, 0x13, 0xF0, 0xA1]; // garbage incl. a fake header
        let mut bad = rx(ty::MODE, &[1]);
        *bad.last_mut().unwrap() ^= 0xFF;
        stream.extend(bad);
        stream.extend(rx(ty::MODE, &[0]));
        let mut d = FrameDecoder::new();
        let frames = d.push(&stream);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, vec![0]);
        assert!(d.bad_checksums >= 1);
    }

    #[test]
    fn ignores_host_frames() {
        let mut d = FrameDecoder::new();
        assert!(d.push(&crate::session(true)).is_empty());
    }
}
