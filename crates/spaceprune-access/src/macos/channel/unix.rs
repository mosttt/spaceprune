//! Bounded IPC framing with the original cancellation and monotonic deadline.

use crate::OperationContext;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    thread,
    time::Duration,
};

const MAX_FRAME: usize = 8 * 1024;

pub(in crate::macos) struct Channel {
    pub(in crate::macos) stream: UnixStream,
    buffered: Vec<u8>,
    received: usize,
    maximum: usize,
}

impl Channel {
    pub(in crate::macos) fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            buffered: Vec::new(),
            received: 0,
            maximum: MAX_FRAME,
        })
    }

    pub(in crate::macos) fn connect(name: &str, context: &OperationContext) -> io::Result<Self> {
        context.check()?;
        Self::new(UnixStream::connect(name)?)
    }

    pub(in crate::macos) fn allow_service_payloads(&mut self) {
        self.maximum = 64 * 1024 * 1024;
    }

    pub(in crate::macos) fn monitor(&self) -> io::Result<Monitor> {
        if !self.buffered.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(Monitor(self.stream.try_clone()?))
    }

    pub(in crate::macos) fn reset_budget(&mut self) {
        self.received = 0;
    }

    pub(in crate::macos) fn write(
        &mut self,
        value: &impl Serialize,
        context: &OperationContext,
    ) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        if bytes.len() > self.maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Native registration frame is too large",
            ));
        }
        let mut remaining = &bytes[..];
        while !remaining.is_empty() {
            context.check()?;
            match self.stream.write(remaining) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => remaining = &remaining[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        context.check()
    }

    pub(in crate::macos) fn write_terminal(
        &mut self,
        value: &impl Serialize,
        context: &OperationContext,
    ) -> io::Result<()> {
        self.write(&serde_json::json!({"native_terminal": value}), context)
    }
    pub(in crate::macos) fn read_terminal<T: DeserializeOwned>(
        &mut self,
        context: &OperationContext,
    ) -> io::Result<T> {
        loop {
            let frame = self.read_frame(context)?;
            if let Some(value) = frame.get("native_terminal") {
                return serde_json::from_value(value.clone())
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
        }
    }
    pub(in crate::macos) fn read<T: DeserializeOwned>(
        &mut self,
        context: &OperationContext,
    ) -> io::Result<T> {
        self.read_with_terminal(context).map(|(value, _)| value)
    }

    pub(in crate::macos) fn read_with_terminal<T: DeserializeOwned>(
        &mut self,
        context: &OperationContext,
    ) -> io::Result<(T, bool)> {
        let frame = self.read_frame(context)?;
        let terminal = frame.get("native_terminal");
        let value = terminal.unwrap_or(&frame);
        serde_json::from_value(value.clone())
            .map(|value| (value, terminal.is_some()))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn read_frame(&mut self, context: &OperationContext) -> io::Result<serde_json::Value> {
        loop {
            context.check()?;
            if let Some(end) = self.buffered.iter().position(|byte| *byte == b'\n') {
                if end + 1 > self.maximum {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let frame: Vec<_> = self.buffered.drain(..=end).collect();
                return serde_json::from_slice(&frame)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
            if self.buffered.len() >= self.maximum {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut buffer = [0; 2048];
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(count) => {
                    self.received += count;
                    if self.received > 3 * self.maximum {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    self.buffered.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

// This adapter is compiled only for Linux tests; it is not a Darwin fallback.
pub(in crate::macos) struct Endpoint {
    _directory: tempfile::TempDir,
    name: String,
    listener: std::os::unix::net::UnixListener,
}
impl Endpoint {
    pub(in crate::macos) fn bind() -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new().prefix("sp-auth-").tempdir()?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let path = directory.path().join("ipc");
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            _directory: directory,
            name: path.to_str().unwrap().into(),
            listener,
        })
    }
    pub(in crate::macos) fn name(&self) -> &str {
        &self.name
    }
    pub(in crate::macos) fn accept(&self) -> io::Result<Option<Channel>> {
        match self.listener.accept() {
            Ok((stream, _)) => Channel::new(stream).map(Some),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}
pub(in crate::macos) struct Monitor(UnixStream);
impl Monitor {
    pub(in crate::macos) fn is_alive(&mut self) -> bool {
        matches!(self.0.read(&mut [0; 1]), Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interaction;

    #[test]
    fn fragmented_frames_keep_boundaries_and_reject_an_oversized_or_unfinished_frame() {
        let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        sender.write_all(b"{\"one\":").unwrap();
        sender.write_all(b"1}\n{\"two\":2}\n").unwrap();
        let mut channel = Channel::new(receiver).unwrap();
        assert_eq!(
            channel.read::<serde_json::Value>(&context).unwrap()["one"],
            1
        );
        assert_eq!(
            channel.read::<serde_json::Value>(&context).unwrap()["two"],
            2
        );
        sender.write_all(&vec![b'x'; MAX_FRAME]).unwrap();
        assert_eq!(
            channel
                .read::<serde_json::Value>(&context)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        sender.write_all(b"{\"one\":").unwrap();
        drop(sender);
        assert_eq!(
            Channel::new(receiver)
                .unwrap()
                .read::<serde_json::Value>(&context)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn a_silent_live_peer_does_not_outlive_cancellation_or_the_original_deadline() {
        let (_sender, receiver) = UnixStream::pair().unwrap();
        let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
        let cancel = context.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            cancel.cancel();
        });
        assert_eq!(
            Channel::new(receiver)
                .unwrap()
                .read::<serde_json::Value>(&context)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        canceller.join().unwrap();
        let (_sender, receiver) = UnixStream::pair().unwrap();
        let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive)
            .child(Duration::from_millis(30));
        assert_eq!(
            Channel::new(receiver)
                .unwrap()
                .read::<serde_json::Value>(&context)
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
