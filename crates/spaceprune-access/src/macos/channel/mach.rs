//! One private Mach conversation; every received message carries a kernel audit token.

mod ports;
use super::framing::{self, Assembler};
use crate::{OperationContext, macos::peer::Peer};
use ports::{DATA, HELLO, MAX_FRAME, ReceivePort, SendRight, TERMINAL, WELCOME};
use serde::{Serialize, de::DeserializeOwned};
use std::{io, sync::Arc, thread, time::Duration};

const PREFIX: &str = "com.spaceprune.registration.";

pub(in crate::macos) struct Endpoint {
    name: String,
    receive: Arc<ReceivePort>,
}
impl Endpoint {
    pub(in crate::macos) fn bind() -> io::Result<Self> {
        let name = format!("{PREFIX}{}", uuid::Uuid::new_v4());
        let receive = Arc::new(ReceivePort::new()?);
        receive.publish(&name)?;
        Ok(Self { name, receive })
    }
    pub(in crate::macos) fn name(&self) -> &str {
        &self.name
    }
    pub(in crate::macos) fn accept(&self) -> io::Result<Option<Channel>> {
        let Some(packet) = self.receive.receive()? else {
            return Ok(None);
        };
        if packet.id != HELLO || !packet.bytes.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let send = packet.reply.ok_or(io::ErrorKind::InvalidData)?;
        Ok(Some(Channel {
            receive: Arc::clone(&self.receive),
            send: Arc::new(send),
            peer: Some(Peer::from_token(packet.token)?),
            received: 0,
            read_message: 0,
            write_message: 0,
            maximum: MAX_FRAME,
        }))
    }
}

pub(in crate::macos) struct ServiceEndpoint(ReceivePort);
impl ServiceEndpoint {
    pub(in crate::macos) fn check_in(kind: crate::macos::ServiceKind) -> io::Result<Self> {
        ReceivePort::check_in(kind.label()).map(Self)
    }

    pub(in crate::macos) fn accept(
        &self,
        context: &OperationContext,
    ) -> io::Result<Option<Channel>> {
        let Some(packet) = self.0.receive()? else {
            return Ok(None);
        };
        if packet.id != HELLO || !packet.bytes.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let channel = Channel {
            receive: Arc::new(ReceivePort::new()?),
            send: Arc::new(packet.reply.ok_or(io::ErrorKind::InvalidData)?),
            peer: Some(Peer::from_token(packet.token)?),
            received: 0,
            read_message: 0,
            write_message: 0,
            maximum: 64 * 1024 * 1024,
        };
        // Each client gets a private receive right. Other clients' messages never enter this
        // conversation. This greeting transfers no authorization; callers still verify code.
        channel.send_frame(WELCOME, &[], Some(&channel.receive), context)?;
        Ok(Some(channel))
    }
}

pub(in crate::macos) struct Channel {
    receive: Arc<ReceivePort>,
    send: Arc<SendRight>,
    peer: Option<Peer>,
    received: usize,
    read_message: u64,
    write_message: u64,
    maximum: usize,
}
impl Channel {
    pub(in crate::macos) fn connect(name: &str, context: &OperationContext) -> io::Result<Self> {
        let nonce = name
            .strip_prefix(PREFIX)
            .ok_or(io::ErrorKind::InvalidInput)?;
        if uuid::Uuid::parse_str(nonce).is_err() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        context.check()?;
        let channel = Self {
            receive: Arc::new(ReceivePort::new()?),
            send: Arc::new(SendRight::lookup(name)?),
            peer: None,
            received: 0,
            read_message: 0,
            write_message: 0,
            maximum: MAX_FRAME,
        };
        channel.send_frame(HELLO, &[], Some(&channel.receive), context)?;
        Ok(channel)
    }
    pub(in crate::macos) fn connect_service(
        kind: crate::macos::ServiceKind,
        context: &OperationContext,
    ) -> io::Result<Self> {
        context.check()?;
        let mut channel = Self {
            receive: Arc::new(ReceivePort::new()?),
            send: Arc::new(SendRight::lookup(kind.label())?),
            peer: None,
            received: 0,
            read_message: 0,
            write_message: 0,
            maximum: 64 * 1024 * 1024,
        };
        channel.send_frame(HELLO, &[], Some(&channel.receive), context)?;
        loop {
            context.check()?;
            if let Some(packet) = channel.receive.receive()? {
                if packet.id != WELCOME || !packet.bytes.is_empty() {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                channel.peer = Some(Peer::from_token(packet.token)?);
                channel.send = Arc::new(packet.reply.ok_or(io::ErrorKind::InvalidData)?);
                return Ok(channel);
            }
            if !channel.is_alive() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(in crate::macos) fn allow_service_payloads(&mut self) {
        self.maximum = 64 * 1024 * 1024;
    }

    pub(in crate::macos) fn is_alive(&self) -> bool {
        self.send.is_alive()
    }

    pub(in crate::macos) fn reset_budget(&mut self) {
        self.received = 0;
    }

    pub(in crate::macos) fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }
    pub(in crate::macos) fn monitor(&self) -> io::Result<Monitor> {
        Ok(Monitor {
            receive: Arc::clone(&self.receive),
            send: Arc::clone(&self.send),
        })
    }
    fn send_frame(
        &self,
        id: i32,
        bytes: &[u8],
        reply: Option<&ReceivePort>,
        context: &OperationContext,
    ) -> io::Result<()> {
        loop {
            context.check()?;
            match self.send.send(id, bytes, reply) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                result => {
                    result?;
                    return context.check();
                }
            }
        }
    }
    pub(in crate::macos) fn write(
        &mut self,
        value: &impl Serialize,
        context: &OperationContext,
    ) -> io::Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len()
            > self
                .maximum
                .min(context.budget.line_bytes)
                .min(context.budget.output_bytes)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.write_message = self
            .write_message
            .checked_add(1)
            .ok_or(io::ErrorKind::InvalidData)?;
        for frame in framing::encode(&bytes, self.write_message) {
            self.send_frame(DATA, &frame, None, context)?;
        }
        Ok(())
    }
    pub(in crate::macos) fn write_terminal(
        &mut self,
        value: &impl Serialize,
        context: &OperationContext,
    ) -> io::Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_FRAME {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.send_frame(TERMINAL, &bytes, None, context)
    }

    // The terminal control frame can follow an interrupted result. Discard data only while
    // closing: every frame still has to come from the original kernel-authenticated peer.
    pub(in crate::macos) fn read_terminal<T: DeserializeOwned>(
        &mut self,
        context: &OperationContext,
    ) -> io::Result<T> {
        loop {
            context.check()?;
            if let Some(packet) = self.checked_packet()? {
                if packet.id == TERMINAL {
                    return serde_json::from_slice(&packet.bytes)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
                continue;
            }
            if !self.send.is_alive() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn checked_packet(&mut self) -> io::Result<Option<ports::Packet>> {
        let Some(packet) = self.receive.receive()? else {
            return Ok(None);
        };
        self.received += packet.bytes.len();
        if !matches!(packet.id, DATA | TERMINAL)
            || packet.reply.is_some()
            || self.received > 3 * self.maximum
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let peer = Peer::from_token(packet.token)?;
        if self
            .peer
            .as_ref()
            .is_some_and(|previous| !previous.same_process(&peer))
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.peer = Some(peer);
        Ok(Some(packet))
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
        let next = self
            .read_message
            .checked_add(1)
            .ok_or(io::ErrorKind::InvalidData)?;
        let maximum = self
            .maximum
            .min(context.budget.line_bytes)
            .min(context.budget.output_bytes);
        let mut assembler = Assembler::new(next, maximum);
        loop {
            context.check()?;
            if let Some(packet) = self.checked_packet()? {
                if packet.id == TERMINAL {
                    return serde_json::from_slice(&packet.bytes)
                        .map(|value| (value, true))
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
                if let Some(bytes) = assembler.push(&packet.bytes)? {
                    self.read_message = next;
                    return serde_json::from_slice(&bytes)
                        .map(|value| (value, false))
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
                }
                continue;
            }
            if !self.send.is_alive() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(in crate::macos) struct Monitor {
    receive: Arc<ReceivePort>,
    send: Arc<SendRight>,
}
impl Monitor {
    pub(in crate::macos) fn is_alive(&mut self) -> bool {
        // The agent expects no further requests while inside a native registration API.
        // A dead send right detects loss of the host's receive right, without trusting a PID.
        self.send.is_alive() && matches!(self.receive.receive(), Ok(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interaction;

    fn context() -> OperationContext {
        OperationContext::workflow(Default::default(), Interaction::NonInteractive)
            .child(Duration::from_secs(2))
    }

    fn pair(context: &OperationContext) -> (Channel, Channel) {
        let endpoint = Endpoint::bind().unwrap();
        let child = Channel::connect(endpoint.name(), context).unwrap();
        let host = endpoint.accept().unwrap().unwrap();
        (host, child)
    }

    #[test]
    fn native_audit_trailers_and_queued_replies_survive_peer_exit() {
        let context = context();
        let (mut host, mut child) = pair(&context);
        assert_eq!(host.peer().unwrap().identity.pid, std::process::id());
        host.write(&"call", &context).unwrap();
        assert_eq!(child.read::<String>(&context).unwrap(), "call");
        assert_eq!(child.peer().unwrap().identity.pid, std::process::id());
        assert!(child.peer().unwrap().same_process(host.peer().unwrap()));
        for value in ["a", "ab", "abc", "abcd"] {
            child.write(&value, &context).unwrap();
        }
        drop(child);
        for value in ["a", "ab", "abc", "abcd"] {
            assert_eq!(host.read::<String>(&context).unwrap(), value);
        }
        assert_eq!(
            host.read::<String>(&context).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn native_large_results_keep_the_same_audited_sender_across_fragments() {
        let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive)
            .child(Duration::from_secs(10));
        let (mut host, mut child) = pair(&context);
        host.maximum = 4 * 1024 * 1024;
        child.maximum = 4 * 1024 * 1024;
        let expected = "metadata-".repeat(128 * 1024);
        let payload = expected.clone();
        let sender_context = context.clone();
        let sender = thread::spawn(move || child.write(&payload, &sender_context).unwrap());
        assert_eq!(host.read::<String>(&context).unwrap(), expected);
        sender.join().unwrap();
    }

    #[test]
    fn native_terminal_receipt_survives_an_abandoned_large_response() {
        for wait_for_terminal in [false, true] {
            let context = context();
            let (mut host, mut child) = pair(&context);
            host.allow_service_payloads();
            child.allow_service_payloads();
            let body = serde_json::to_vec(&"x".repeat(MAX_FRAME * 2)).unwrap();
            let first = framing::encode(&body, 1).next().unwrap();
            child.send_frame(DATA, &first, None, &context).unwrap();
            child
                .write_terminal(&serde_json::json!({"stopped": true}), &context)
                .unwrap();
            drop(child);
            let result: serde_json::Value = if wait_for_terminal {
                host.read_terminal(&context).unwrap()
            } else {
                let (value, terminal) = host.read_with_terminal(&context).unwrap();
                assert!(terminal);
                value
            };
            assert_eq!(result, serde_json::json!({"stopped": true}));
        }
    }

    #[test]
    fn native_frame_limit_queue_deadline_and_host_monitor_are_enforced() {
        let context = context();
        let (mut host, mut child) = pair(&context);
        assert_eq!(
            host.write(&"x".repeat(MAX_FRAME), &context)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        host.write(&"call", &context).unwrap();
        child.read::<String>(&context).unwrap();
        let mut monitor = child.monitor().unwrap();
        assert!(monitor.is_alive());
        host.write(&"unexpected second call", &context).unwrap();
        assert!(!monitor.is_alive());
        drop(host);
        assert!(!monitor.is_alive());
        let (mut host, _child) = pair(&context);
        let deadline = context.child(Duration::from_millis(50));
        let failure = (0..100)
            .find_map(|_| host.write(&"fill queue", &deadline).err())
            .unwrap();
        assert_eq!(failure.kind(), io::ErrorKind::TimedOut);
    }
}
