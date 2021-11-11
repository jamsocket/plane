use crate::parse_helpers::{colon, consume_until_newline, decimal, hex_u16, hex_u8, whitespace};

/// Parser for /proc/net/tcp files, which are documented
/// [here](https://www.kernel.org/doc/Documentation/networking/proc_net_tcp.txt)

type Ip = [u8; 4];

/// TCP connection states enum, from
/// [here](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/net/tcp_states.h).
#[derive(PartialEq, Debug, Copy, Clone)]
#[allow(unused)]
pub enum TcpConnectionState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    NewSynRecv,
}

pub struct InvalidConnectionState;

impl TryFrom<u8> for TcpConnectionState {
    type Error = InvalidConnectionState;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(TcpConnectionState::Established),
            0x02 => Ok(TcpConnectionState::SynSent),
            0x03 => Ok(TcpConnectionState::SynRecv),
            0x04 => Ok(TcpConnectionState::FinWait1),
            0x05 => Ok(TcpConnectionState::FinWait2),
            0x06 => Ok(TcpConnectionState::TimeWait),
            0x07 => Ok(TcpConnectionState::Close),
            0x08 => Ok(TcpConnectionState::CloseWait),
            0x09 => Ok(TcpConnectionState::LastAck),
            0x0A => Ok(TcpConnectionState::Listen),
            0x0B => Ok(TcpConnectionState::Closing),
            0x0C => Ok(TcpConnectionState::NewSynRecv),
            _ => Err(InvalidConnectionState),
        }
    }
}

/// Represents an actual port or the "*" wildcard port, which Linux
/// represents with port number 0.
#[derive(PartialEq, Debug)]
pub enum Port {
    Wildcard,
    Port(u16),
}

/// Represents an address, which is an IP address paired with a port number.
#[derive(PartialEq, Debug)]
pub struct Address {
    pub ip: Ip,
    pub port: Port,
}

/// Represents metadata about a TCP connection.
#[derive(PartialEq, Debug)]
pub struct TcpConnection {
    pub local_address: Address,
    pub remote_address: Address,
    pub state: TcpConnectionState,
}

/// Parse a port number from a byte string, represented as four hex digits.
fn parse_port(data: &[u8]) -> Option<(&[u8], Port)> {
    let (data, port) = hex_u16(data)?;

    let port = if port == 0 {
        Port::Wildcard
    } else {
        Port::Port(port)
    };

    Some((data, port))
}

/// Parse an Address from a byte string, represented as eight hex digits
/// for the IP followed by a colon and four hex digits for the port.
fn parse_address(data: &[u8]) -> Option<(&[u8], Address)> {
    let (data, p4) = hex_u8(data)?;
    let (data, p3) = hex_u8(data)?;
    let (data, p2) = hex_u8(data)?;
    let (data, p1) = hex_u8(data)?;

    let data = colon(data)?;

    let (data, port) = parse_port(data)?;

    let address = Address {
        ip: [p1, p2, p3, p4],
        port,
    };

    Some((data, address))
}

/// Parse a TcpConnection from the beginning of a byte string,
/// in /proc/net/tcp format.
fn parse_connection(data: &[u8]) -> Option<(&[u8], TcpConnection)> {
    // The entry number is provided in decimal, (possibly) with leading spaces,
    // and followed by a colon.
    let data = whitespace(data).unwrap_or(data);
    let (data, _entry_number) = decimal(data)?;
    let data = colon(data)?;
    let data = whitespace(data)?;

    // The local address.
    let (data, local_address) = parse_address(data)?;
    let data = whitespace(data)?;

    // The remote address.
    let (data, remote_address) = parse_address(data)?;
    let data = whitespace(data)?;

    // The connection state is provided as a hex byte.
    let (data, state) = hex_u8(data)?;
    let data = whitespace(data)?;

    // We don't need anything that comes after this point on this line.
    let data = consume_until_newline(data);

    // Convert the state into our enum representation.
    let state = TcpConnectionState::try_from(state).ok()?;

    Some((
        data,
        TcpConnection {
            local_address,
            remote_address,
            state,
        },
    ))
}

/// Parse all TcpConnections from a byte string representing an entire
/// /proc/net/tcp buffer.
pub fn parse_connections(data: &[u8]) -> Option<Vec<TcpConnection>> {
    let mut data = consume_until_newline(data);

    let mut result: Vec<TcpConnection> = Vec::new();

    while let Some((new_data, connection)) = parse_connection(data) {
        data = new_data;
        result.push(connection);
    }

    // If there is data left over, we don't crash but do log a warning.
    if !data.is_empty() {
        let next_began = std::str::from_utf8(&data[..30]).unwrap_or("(could not parse as utf-8)");
        log::warn!("Did not fully consume connections due to parse error. Consumed {}; next line began: {}",
            result.len(),
            next_began
        );

        return None;
    }

    Some(result)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_single() {
        let input_data = include_bytes!("../test_data/test1.txt");

        let expected = Some(vec![TcpConnection {
            local_address: Address {
                ip: [0, 0, 0, 0],
                port: Port::Port(8080),
            },
            remote_address: Address {
                ip: [0, 0, 0, 0],
                port: Port::Wildcard,
            },
            state: TcpConnectionState::Listen,
        }]);

        assert_eq!(expected, parse_connections(input_data));
    }

    #[test]
    fn test_parse_time_wait() {
        let input_data = include_bytes!("../test_data/test2.txt");

        let expected = Some(vec![
            TcpConnection {
                local_address: Address {
                    ip: [0, 0, 0, 0],
                    port: Port::Port(8080),
                },
                remote_address: Address {
                    ip: [0, 0, 0, 0],
                    port: Port::Wildcard,
                },
                state: TcpConnectionState::Listen,
            },
            TcpConnection {
                local_address: Address {
                    ip: [172, 17, 0, 7],
                    port: Port::Port(8080),
                },
                remote_address: Address {
                    ip: [172, 17, 0, 1],
                    port: Port::Port(4977),
                },
                state: TcpConnectionState::TimeWait,
            },
        ]);

        assert_eq!(expected, parse_connections(input_data));
    }

    #[test]
    fn test_parse_established() {
        let input_data = include_bytes!("../test_data/test3.txt");

        let expected = Some(vec![
            TcpConnection {
                local_address: Address {
                    ip: [0, 0, 0, 0],
                    port: Port::Port(8080),
                },
                remote_address: Address {
                    ip: [0, 0, 0, 0],
                    port: Port::Wildcard,
                },
                state: TcpConnectionState::Listen,
            },
            TcpConnection {
                local_address: Address {
                    ip: [172, 17, 0, 7],
                    port: Port::Port(8080),
                },
                remote_address: Address {
                    ip: [172, 17, 0, 1],
                    port: Port::Port(18037),
                },
                state: TcpConnectionState::Established,
            },
        ]);

        assert_eq!(expected, parse_connections(input_data));
    }
}
