use std::{
    collections::VecDeque,
    io,
    net::{SocketAddr, UdpSocket},
    time::Duration,
};

use renet::ClientId;
use renet_netcode::{ConnectToken, NETCODE_KEY_BYTES};
use snow::{Builder, params::NoiseParams};

use crate::{
    NetworkError, NetworkResult,
    protocol::{
        CONNECT_TOKEN_LIFETIME_SECONDS, MAX_BOOTSTRAP_PACKET_SIZE, NOISE_PATTERN, PROTOCOL_HEADER,
        PROTOCOL_ID,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bootstrap {
    pub address: SocketAddr,
    pub public_key: [u8; 32],
}

struct CachedResponse {
    client_address: SocketAddr,
    request: Vec<u8>,
    response: Vec<u8>,
    expires_at: Duration,
}

pub(crate) struct BootstrapServer {
    socket: UdpSocket,
    private_key: [u8; 32],
    bootstrap: Bootstrap,
    game_address: SocketAddr,
    netcode_key: [u8; NETCODE_KEY_BYTES],
    connection_timeout_seconds: i32,
    next_client_id: ClientId,
    response_cache: VecDeque<CachedResponse>,
    response_cache_capacity: usize,
}

impl BootstrapServer {
    pub(crate) fn bind(
        address: SocketAddr,
        game_address: SocketAddr,
        netcode_key: [u8; NETCODE_KEY_BYTES],
        connection_timeout_seconds: i32,
        max_clients: usize,
    ) -> NetworkResult<Self> {
        let socket = UdpSocket::bind(address)?;
        socket.set_nonblocking(true)?;

        let keypair = Builder::new(noise_params()).generate_keypair()?;
        let private_key = keypair
            .private
            .try_into()
            .map_err(|_| NetworkError::InvalidBootstrapResponse)?;
        let public_key = keypair
            .public
            .try_into()
            .map_err(|_| NetworkError::InvalidBootstrapResponse)?;

        Ok(Self {
            socket,
            private_key,
            bootstrap: Bootstrap {
                address,
                public_key,
            },
            game_address,
            netcode_key,
            connection_timeout_seconds,
            next_client_id: 1,
            response_cache: VecDeque::new(),
            response_cache_capacity: max_clients,
        })
    }

    pub(crate) fn bootstrap(&self) -> Bootstrap {
        self.bootstrap
    }

    pub(crate) fn receive_requests(&mut self, current_time: Duration) -> NetworkResult<()> {
        while self
            .response_cache
            .front()
            .is_some_and(|response| response.expires_at <= current_time)
        {
            self.response_cache.pop_front();
        }

        let mut request_packet = [0; MAX_BOOTSTRAP_PACKET_SIZE];

        loop {
            let (request_size, client_address) = match self.socket.recv_from(&mut request_packet) {
                Ok(received) => received,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };

            let Some(request) = request_packet[..request_size].strip_prefix(&PROTOCOL_HEADER)
            else {
                continue;
            };

            if let Some(cached) = self
                .response_cache
                .iter()
                .find(|cached| cached.client_address == client_address && cached.request == request)
            {
                self.socket.send_to(&cached.response, client_address)?;
                continue;
            }

            let mut handshake = Builder::new(noise_params())
                .local_private_key(&self.private_key)?
                .build_responder()?;
            let mut payload = [];
            match handshake.read_message(request, &mut payload) {
                Ok(0) => {}
                Ok(_) | Err(_) => continue,
            }

            let client_id = self.next_client_id;
            self.next_client_id = self
                .next_client_id
                .checked_add(1)
                .ok_or(NetworkError::ClientIdExhausted)?;

            let token = ConnectToken::generate(
                current_time,
                PROTOCOL_ID,
                CONNECT_TOKEN_LIFETIME_SECONDS,
                client_id,
                self.connection_timeout_seconds,
                vec![self.game_address],
                None,
                &self.netcode_key,
            )?;
            let mut token_bytes = Vec::with_capacity(MAX_BOOTSTRAP_PACKET_SIZE);
            token.write(&mut token_bytes)?;

            let mut response_packet = [0; MAX_BOOTSTRAP_PACKET_SIZE];
            response_packet[..PROTOCOL_HEADER.len()].copy_from_slice(&PROTOCOL_HEADER);
            let response_size = handshake
                .write_message(&token_bytes, &mut response_packet[PROTOCOL_HEADER.len()..])?
                + PROTOCOL_HEADER.len();
            self.socket
                .send_to(&response_packet[..response_size], client_address)?;

            if self.response_cache.len() == self.response_cache_capacity {
                self.response_cache.pop_front();
            }
            self.response_cache.push_back(CachedResponse {
                client_address,
                request: request.to_vec(),
                response: response_packet[..response_size].to_vec(),
                expires_at: current_time + Duration::from_secs(CONNECT_TOKEN_LIFETIME_SECONDS),
            });
        }
    }
}

pub(crate) fn noise_params() -> NoiseParams {
    NOISE_PATTERN.parse().expect("valid Noise protocol name")
}
