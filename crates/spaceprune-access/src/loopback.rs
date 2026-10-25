//! Exclusive Windows loopback listeners without exposing raw sockets to callers.

use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    os::windows::io::AsRawSocket,
};
use windows_sys::Win32::Networking::WinSock::{
    SO_EXCLUSIVEADDRUSE, SOCKET_ERROR, SOL_SOCKET, WSAGetLastError, setsockopt,
};

/// Bind an exclusive, nonblocking TCP listener to `127.0.0.1` on an available port.
///
/// Windows address exclusivity is enabled before binding so another socket cannot
/// take the same endpoint using `SO_REUSEADDR`. Any setup failure closes the socket
/// and returns its OS error; the listener never falls back to shared or public binding.
/// The caller owns the listener and must drop it when its callback attempt ends.
pub fn bind_exclusive_listener() -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    set_exclusive_address_use(&socket)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())?;
    socket.listen(8)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[allow(unsafe_code)]
fn set_exclusive_address_use(socket: &Socket) -> io::Result<()> {
    let enabled: i32 = 1;
    // SAFETY: Socket owns a live IPv4 TCP socket and initialized Winsock. The BOOL
    // buffer remains readable for this synchronous call, with its exact byte length.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as _,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw const enabled).cast(),
            size_of::<i32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        // SAFETY: WSAGetLastError takes no pointers and reads this thread's Winsock error.
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use windows_sys::Win32::Networking::WinSock::{WSAEACCES, WSAEADDRINUSE};

    #[test]
    fn listener_is_nonblocking_and_accepts_only_on_an_ephemeral_loopback_address() {
        let listener = bind_exclusive_listener().unwrap();
        let address = listener.local_addr().unwrap();
        assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(address.port(), 0);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        let client = TcpStream::connect(address).unwrap();
        let (_accepted, peer) = listener.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
    }

    #[test]
    fn another_socket_cannot_rebind_the_endpoint_even_with_reuseaddr() {
        let listener = bind_exclusive_listener().unwrap();
        let address = listener.local_addr().unwrap();
        for reuse in [false, true] {
            let competing = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
            competing.set_reuse_address(reuse).unwrap();
            let error = competing.bind(&address.into()).unwrap_err();
            assert!(
                matches!(error.raw_os_error(), Some(WSAEACCES | WSAEADDRINUSE)),
                "reuseaddr={reuse}: {error}"
            );
        }
    }

    #[test]
    fn dropping_the_listener_releases_the_exclusive_endpoint() {
        let listener = bind_exclusive_listener().unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let next = TcpListener::bind(address).unwrap();
        assert_eq!(next.local_addr().unwrap(), address);
    }
}
