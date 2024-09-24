use dynamic_proxy::server::{HttpsConfig, SimpleHttpServer};
use plane::{
    protocol::{RouteInfoRequest, RouteInfoResponse},
    proxy::proxy_server::ProxyState,
};
use std::net::SocketAddr;
use tokio::{net::TcpListener, sync::mpsc};

pub struct MockProxy {
    proxy_state: ProxyState,
    route_info_request_receiver: mpsc::Receiver<RouteInfoRequest>,
    addr: SocketAddr,
    _server: SimpleHttpServer,
}

#[allow(unused)]
impl MockProxy {
    pub async fn new() -> Self {
        let proxy_state = ProxyState::new();
        let (route_info_request_sender, route_info_request_receiver) = mpsc::channel(1);

        proxy_state.inner.route_map.set_sender(move |m| {
            route_info_request_sender
                .try_send(m)
                .expect("Failed to send route info request");
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind listener");
        let addr = listener.local_addr().expect("Failed to get local address");

        let server = SimpleHttpServer::new(proxy_state.clone(), listener, HttpsConfig::http())
            .expect("Failed to create server");

        Self {
            proxy_state,
            route_info_request_receiver,
            addr,
            _server: server,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub async fn recv_route_info_request(&mut self) -> RouteInfoRequest {
        self.route_info_request_receiver
            .recv()
            .await
            .expect("Failed to receive route info request")
    }

    pub async fn expect_no_route_info_request(&mut self) {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        assert!(
            self.route_info_request_receiver.is_empty(),
            "Expected no route info request, but got: {}",
            self.route_info_request_receiver.len()
        );
    }

    pub async fn send_route_info_response(&mut self, response: RouteInfoResponse) {
        self.proxy_state.inner.route_map.receive(response);
    }
}
