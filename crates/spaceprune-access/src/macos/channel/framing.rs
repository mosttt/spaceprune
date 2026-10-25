//! Bounded, ordered inline fragments. Mach supplies the sender identity for every fragment.
use std::io;

pub(super) const MAX_FRAME: usize = 8 * 1024;
const HEADER: usize = 20;
const CHUNK: usize = MAX_FRAME - HEADER;

pub(super) fn encode(bytes: &[u8], message: u64) -> impl Iterator<Item = Vec<u8>> + '_ {
    bytes.chunks(CHUNK).enumerate().map(move |(index, chunk)| {
        let mut frame = Vec::with_capacity(HEADER + chunk.len());
        frame.extend_from_slice(&message.to_le_bytes());
        frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        frame.extend_from_slice(&((index * CHUNK) as u32).to_le_bytes());
        frame.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        frame.extend_from_slice(chunk);
        frame
    })
}

pub(super) struct Assembler {
    message: u64,
    maximum: usize,
    length: Option<usize>,
    bytes: Vec<u8>,
}
impl Assembler {
    pub(super) fn new(message: u64, maximum: usize) -> Self {
        Self {
            message,
            maximum,
            length: None,
            bytes: Vec::new(),
        }
    }
    pub(super) fn push(&mut self, frame: &[u8]) -> io::Result<Option<Vec<u8>>> {
        if frame.len() < HEADER || frame.len() > MAX_FRAME {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let message = u64::from_le_bytes(frame[..8].try_into().unwrap());
        let length = u32::from_le_bytes(frame[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(frame[12..16].try_into().unwrap()) as usize;
        let count = u32::from_le_bytes(frame[16..20].try_into().unwrap()) as usize;
        if message != self.message
            || length == 0
            || length > self.maximum
            || offset != self.bytes.len()
            || count == 0
            || count > CHUNK
            || offset.checked_add(count).is_none_or(|end| end > length)
            || frame.len() != (HEADER + count).next_multiple_of(4)
            || self.length.is_some_and(|expected| expected != length)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.length = Some(length);
        self.bytes.extend_from_slice(&frame[HEADER..HEADER + count]);
        Ok((self.bytes.len() == length).then(|| std::mem::take(&mut self.bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
        bytes.resize(bytes.len().next_multiple_of(4), b' ');
        bytes
    }
    #[test]
    fn large_binary_frames_are_exact_and_reject_replay_gaps_and_budget_overruns() {
        let bytes: Vec<u8> = (0..CHUNK * 3 + 7).map(|index| index as u8).collect();
        let frames: Vec<_> = encode(&bytes, 17).map(padded).collect();
        let mut receiver = Assembler::new(17, bytes.len());
        for frame in &frames[..3] {
            assert!(receiver.push(frame).unwrap().is_none());
        }
        assert_eq!(receiver.push(&frames[3]).unwrap(), Some(bytes.clone()));
        assert!(Assembler::new(18, bytes.len()).push(&frames[0]).is_err());
        assert!(
            Assembler::new(17, bytes.len() - 1)
                .push(&frames[0])
                .is_err()
        );
        assert!(Assembler::new(17, bytes.len()).push(&frames[1]).is_err());
        let mut receiver = Assembler::new(17, bytes.len());
        receiver.push(&frames[0]).unwrap();
        assert!(receiver.push(&frames[0]).is_err());
        let mut altered = frames[1].clone();
        altered[8..12].copy_from_slice(&(bytes.len() as u32 - 1).to_le_bytes());
        assert!(receiver.push(&altered).is_err());
    }
}
