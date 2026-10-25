//! Small RAII boundary around Mach rights and inline messages (macOS 10.13+).
#![allow(unsafe_code)]

use mach2::{
    bootstrap::{bootstrap_check_in, bootstrap_look_up, bootstrap_port, bootstrap_register},
    kern_return::KERN_SUCCESS,
    mach_port::{
        mach_port_allocate, mach_port_deallocate, mach_port_destroy, mach_port_insert_right,
    },
    message::*,
    port::{MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE, MACH_PORT_RIGHT_SEND, mach_port_t},
    traps::mach_task_self,
};
use std::{ffi::CString, io, mem::size_of};

pub(super) use super::super::framing::MAX_FRAME;
pub(super) const HELLO: i32 = 0x5350_0200;
pub(super) const DATA: i32 = HELLO + 1;
pub(super) const WELCOME: i32 = HELLO + 2;
pub(super) const TERMINAL: i32 = HELLO + 3;
const HEADER_SIZE: usize = size_of::<mach_msg_header_t>();
const TRAILER_SIZE: usize = size_of::<mach_msg_audit_trailer_t>();

unsafe extern "C" {
    fn mach_port_type(task: mach_port_t, name: mach_port_t, kind: *mut u32) -> i32;
}

fn result(code: i32) -> io::Result<()> {
    if code == KERN_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::other(format!("Mach IPC failed ({code:#x})")))
    }
}

pub(super) struct ReceivePort(mach_port_t);
impl ReceivePort {
    pub(super) fn new() -> io::Result<Self> {
        let mut port = MACH_PORT_NULL;
        // SAFETY: The kernel initializes one receive right owned by this wrapper.
        result(unsafe {
            mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &mut port)
        })?;
        Ok(Self(port))
    }

    pub(super) fn check_in(name: &str) -> io::Result<Self> {
        let name = CString::new(name)?;
        let mut port = MACH_PORT_NULL;
        // SAFETY: launchd supplies the receive right reserved for this fixed signed service.
        result(unsafe { bootstrap_check_in(bootstrap_port, name.as_ptr(), &mut port) })?;
        Ok(Self(port))
    }

    pub(super) fn publish(&self, name: &str) -> io::Result<()> {
        let name = CString::new(name)?;
        // SAFETY: This wrapper owns the receive right; create a temporary local send reference.
        result(unsafe {
            mach_port_insert_right(mach_task_self(), self.0, self.0, MACH_MSG_TYPE_MAKE_SEND)
        })?;
        let send = SendRight(self.0);
        // SAFETY: launchd copies the send right. The legacy ABI takes a mutable C string but
        // does not modify it. This unique rendezvous is removed when its receive right dies.
        let status =
            unsafe { bootstrap_register(bootstrap_port, name.as_ptr().cast_mut(), send.0) };
        result(status)
    }

    pub(super) fn receive(&self) -> io::Result<Option<Packet>> {
        let mut buffer = Buffer::new();
        // SAFETY: Buffer is C-aligned and sized for the entire inline frame plus audit trailer.
        // No LARGE option: an oversized message is discarded by the kernel, not left queued.
        let status = unsafe {
            mach_msg(
                &mut buffer.header,
                MACH_RCV_MSG | MACH_RCV_TIMEOUT | ((MACH_RCV_TRAILER_AUDIT as i32) << 24),
                0,
                size_of::<Buffer>() as u32,
                self.0,
                0,
                MACH_PORT_NULL,
            )
        };
        if status == MACH_RCV_TIMED_OUT {
            return Ok(None);
        }
        if status != MACH_MSG_SUCCESS {
            if status & !MACH_MSG_MASK == MACH_RCV_BODY_ERROR {
                // A partial body copyout still transfers the successfully copied rights.
                drop(Received(buffer));
            }
            return result(status).map(|()| None);
        }
        let mut message = Received(buffer);
        let header = message.0.header;
        let size = header.msgh_size as usize;
        if header.msgh_bits & MACH_MSGH_BITS_COMPLEX != 0
            || header.msgh_voucher_port != MACH_PORT_NULL
            || !(HEADER_SIZE..=HEADER_SIZE + MAX_FRAME).contains(&size)
            || !size.is_multiple_of(4)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let offset = size - HEADER_SIZE;
        // SAFETY: Size was checked against the fixed payload capacity. Audit trailers use
        // four-byte Mach alignment; read_unaligned avoids imposing any stronger alignment.
        let trailer = unsafe {
            message
                .0
                .payload
                .as_ptr()
                .add(offset)
                .cast::<mach_msg_audit_trailer_t>()
                .read_unaligned()
        };
        if trailer.msgh_trailer_type != MACH_MSG_TRAILER_FORMAT_0
            || trailer.msgh_trailer_size as usize != TRAILER_SIZE
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let disposition = header.msgh_bits & MACH_MSGH_BITS_REMOTE_MASK;
        let reply = if header.msgh_remote_port == MACH_PORT_NULL && disposition == 0 {
            None
        } else if header.msgh_remote_port != MACH_PORT_NULL
            && disposition == MACH_MSG_TYPE_PORT_SEND
        {
            message.0.header.msgh_remote_port = MACH_PORT_NULL;
            message.0.header.msgh_bits &= !MACH_MSGH_BITS_REMOTE_MASK;
            Some(SendRight(header.msgh_remote_port))
        } else {
            return Err(io::ErrorKind::InvalidData.into());
        };
        Ok(Some(Packet {
            id: header.msgh_id,
            bytes: message.0.payload[..offset].to_vec(),
            token: trailer.msgh_audit,
            reply,
        }))
    }
}
impl Drop for ReceivePort {
    fn drop(&mut self) {
        // SAFETY: Only this wrapper owns the receive right. Destroying it also releases queued
        // messages and makes all remote send rights dead, including the launchd registration.
        unsafe {
            mach_port_destroy(mach_task_self(), self.0);
        }
    }
}

pub(super) struct SendRight(mach_port_t);
impl SendRight {
    pub(super) fn lookup(name: &str) -> io::Result<Self> {
        let name = CString::new(name)?;
        let mut port = MACH_PORT_NULL;
        // SAFETY: A successful lookup returns one owned send reference.
        result(unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port) })?;
        Ok(Self(port))
    }

    pub(super) fn is_alive(&self) -> bool {
        let mut kind = 0;
        // SAFETY: The wrapper keeps the port name alive, even after it becomes a dead name.
        (unsafe { mach_port_type(mach_task_self(), self.0, &mut kind) == KERN_SUCCESS })
            && kind & (1 << (MACH_PORT_RIGHT_SEND + 16)) != 0
    }

    pub(super) fn send(
        &self,
        id: i32,
        bytes: &[u8],
        reply: Option<&ReceivePort>,
    ) -> io::Result<()> {
        if bytes.len() > MAX_FRAME {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut buffer = Buffer::new();
        // JSON allows trailing spaces; round the inline frame to Mach's four-byte alignment.
        let length = bytes.len().next_multiple_of(4);
        buffer.payload[..length].fill(b' ');
        buffer.payload[..bytes.len()].copy_from_slice(bytes);
        buffer.header = mach_msg_header_t {
            msgh_bits: MACH_MSGH_BITS(
                MACH_MSG_TYPE_COPY_SEND,
                if reply.is_some() {
                    MACH_MSG_TYPE_MAKE_SEND
                } else {
                    0
                },
            ),
            msgh_size: (HEADER_SIZE + length) as u32,
            msgh_remote_port: self.0,
            msgh_local_port: reply.map_or(MACH_PORT_NULL, |port| port.0),
            msgh_voucher_port: MACH_PORT_NULL,
            msgh_id: id,
        };
        // SAFETY: All rights remain owned locally; the kernel copies/makes only send rights.
        // A short timeout keeps a full peer queue bounded by the original operation's deadline.
        let status = unsafe {
            mach_msg(
                &mut buffer.header,
                MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                (HEADER_SIZE + length) as u32,
                0,
                MACH_PORT_NULL,
                10,
                MACH_PORT_NULL,
            )
        };
        if status == MACH_MSG_SUCCESS {
            return Ok(());
        }
        // After copy-in, failed sends pseudo-copyout rights as MOVE_SEND. Release those new
        // references, not the original COPY_SEND/MAKE_SEND rights from pre-copy-in failures.
        for (port, disposition) in [
            (
                buffer.header.msgh_remote_port,
                buffer.header.msgh_bits & MACH_MSGH_BITS_REMOTE_MASK,
            ),
            (
                buffer.header.msgh_local_port,
                (buffer.header.msgh_bits & MACH_MSGH_BITS_LOCAL_MASK) >> 8,
            ),
        ] {
            if port != MACH_PORT_NULL && disposition == MACH_MSG_TYPE_PORT_SEND {
                drop(SendRight(port));
            }
        }
        if status & !MACH_MSG_MASK == MACH_SEND_TIMED_OUT {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            result(status)
        }
    }
}
impl Drop for SendRight {
    fn drop(&mut self) {
        // SAFETY: Release exactly one owned send/dead-name reference, never a receive right.
        unsafe {
            mach_port_deallocate(mach_task_self(), self.0);
        }
    }
}

pub(super) struct Packet {
    pub(super) id: i32,
    pub(super) bytes: Vec<u8>,
    pub(super) token: audit_token_t,
    pub(super) reply: Option<SendRight>,
}

#[repr(C)]
struct Buffer {
    header: mach_msg_header_t,
    payload: [u8; MAX_FRAME + TRAILER_SIZE],
}
impl Buffer {
    fn new() -> Self {
        Self {
            header: mach_msg_header_t::default(),
            payload: [0; MAX_FRAME + TRAILER_SIZE],
        }
    }
}
struct Received(Buffer);
impl Drop for Received {
    fn drop(&mut self) {
        // SAFETY: This is a successfully received kernel message. Destroy unclaimed rights,
        // including descriptors in rejected complex messages; claimed reply rights are cleared.
        unsafe {
            mach_msg_destroy(&mut self.0.header);
        }
    }
}
