use anyhow::anyhow;
use netlink_packet_sock_diag::{
    constants::*,
    inet::{ExtensionFlags, InetRequest, SocketId, StateFlags},
    NetlinkHeader, NetlinkMessage, NetlinkPayload, SockDiagMessage,
};
use netlink_sys::{
    protocols::NETLINK_SOCK_DIAG, AsyncSocket, AsyncSocketExt, SocketAddr, TokioSocket,
};
use std::time::Duration;

pub async fn wait_for_ready_port(port: Option<u16>) -> anyhow::Result<u16> {
    loop {
        let listening_ports = get_listen_ports().await?;

        if let Some(port) = port {
            // Port is provided, only wait for a certain port.
            if listening_ports.contains(&port) {
                return Ok(port);
            }
        } else {
            if listening_ports.len() > 1 {
                return Err(anyhow!(
                    "Found listeners on multiple ports, not sure which to choose."
                ));
            } else if let Some(port) = listening_ports.first() {
                return Ok(*port);
            } else {
                continue;
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn get_listen_ports() -> anyhow::Result<Vec<u16>> {
    let mut result = Vec::new();
    // Collect IPV4 ports.
    result.extend(get_listen_ports_for_socket_id(false).await?.into_iter());
    // Collect IPV6 ports.
    result.extend(get_listen_ports_for_socket_id(true).await?.into_iter());
    Ok(result)
}

pub async fn get_listen_ports_for_socket_id(ipv6: bool) -> anyhow::Result<Vec<u16>> {
    let mut socket = TokioSocket::new(NETLINK_SOCK_DIAG)
        .map_err(|e| anyhow!("Couldn't open netlink socket. {:?}", e))?;

    let family = if ipv6 { AF_INET6 } else { AF_INET };

    let mut packet = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_REQUEST | NLM_F_DUMP,
            ..Default::default()
        },
        payload: SockDiagMessage::InetRequest(InetRequest {
            family,
            protocol: IPPROTO_TCP.into(),
            extensions: ExtensionFlags::empty(),
            states: StateFlags::LISTEN,
            socket_id: SocketId::new_v4(),
        })
        .into(),
    };

    packet.finalize();

    let mut buf = vec![0; packet.header.length as usize];

    if buf.len() < packet.buffer_len() {
        return Err(anyhow!("Buffer is not big enough for packet."));
    }

    packet.serialize(&mut buf[..]);
    let kernel_unicast: SocketAddr = SocketAddr::new(0, 0);

    socket
        .send_to(&buf[..], &kernel_unicast)
        .await
        .map_err(|e| anyhow!("Error sending netlink packet. {:?}", e))?;

    let mut listening_ports: Vec<u16> = Vec::new();

    if let Ok((buffer, _)) = socket.recv_from_full().await {
        let mut offset = 0;
        loop {
            let bytes = &buffer[offset..];

            let rx_packet = <NetlinkMessage<SockDiagMessage>>::deserialize(bytes)
                .map_err(|e| anyhow!("Deserialization error. {:?}", e))?;

            match rx_packet.payload {
                NetlinkPayload::Noop | NetlinkPayload::Ack(_) => {}
                NetlinkPayload::InnerMessage(SockDiagMessage::InetResponse(response)) => {
                    let socket_id = response.header.socket_id;
                    if !socket_id.source_address.is_loopback() {
                        // Ignore loopback listeners.
                        listening_ports.push(socket_id.source_port);
                    }
                }
                NetlinkPayload::Done => {
                    break;
                }
                NetlinkPayload::Error(e) => {
                    return Err(anyhow!("Unknown error with netlink: {:?}", e));
                }
                NetlinkPayload::Overrun(e) => {
                    return Err(anyhow!("Overrun error with netlink: {:?}", e));
                }
                NetlinkPayload::InnerMessage(msg) => {
                    return Err(anyhow!(
                        "Unexpectedly receieved non-InetResponse InnerMessage {:?}.",
                        msg
                    ));
                }
            }

            offset += rx_packet.header.length as usize;
            if rx_packet.header.length == 0 || offset == buffer.len() {
                break;
            }
        }
    }

    Ok(listening_ports)
}
