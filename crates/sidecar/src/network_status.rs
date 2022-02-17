use anyhow::anyhow;
use netlink_packet_sock_diag::{
    constants::*,
    inet::{ExtensionFlags, InetRequest, SocketId, StateFlags},
    NetlinkHeader, NetlinkMessage, NetlinkPayload, SockDiagMessage,
};
use netlink_sys::{
    protocols::NETLINK_SOCK_DIAG, AsyncSocket, AsyncSocketExt, SocketAddr, TokioSocket,
};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

pub async fn wait_for_ready_port(port: u16) -> anyhow::Result<()> {
    loop {
        let ready = check_for_ready_port(port, false).await?;

        if ready {
            return Ok(());
        }

        let ready = check_for_ready_port(port, true).await?;

        if ready {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn check_for_ready_port(port: u16, ipv6: bool) -> anyhow::Result<bool> {
    let mut socket = TokioSocket::new(NETLINK_SOCK_DIAG)
        .map_err(|e| anyhow!("Couldn't open netlink socket. {:?}", e))?;

    let socket_id = if ipv6 {
        SocketId {
            source_port: port,
            source_address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            ..SocketId::new_v6()
        }
    } else {
        SocketId {
            source_port: port,
            source_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            ..SocketId::new_v4()
        }
    };

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
            socket_id,
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

    if let Ok((buffer, _)) = socket.recv_from_full().await {
        let mut offset = 0;
        loop {
            let bytes = &buffer[offset..];

            let rx_packet = <NetlinkMessage<SockDiagMessage>>::deserialize(bytes)
                .map_err(|e| anyhow!("Deserialization error. {:?}", e))?;

            match rx_packet.payload {
                NetlinkPayload::Noop | NetlinkPayload::Ack(_) => {}
                NetlinkPayload::InnerMessage(SockDiagMessage::InetResponse(_response)) => {
                    // todo: check response
                    return Ok(true);
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

    Ok(false)
}
