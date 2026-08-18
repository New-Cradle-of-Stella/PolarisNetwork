use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use renet::{ConnectionConfig, RenetClient};
use renet_netcode::{
    ClientAuthentication, ConnectToken, NetcodeClientTransport, NetcodeError, NetcodeTransportError,
};
use snow::{Builder, HandshakeState};

use crate::{
    Bootstrap, Channel, DisconnectReason, NetworkError, NetworkResult,
    bootstrap::noise_params,
    protocol::{BOOTSTRAP_MAX_PACKET_SIZE, PROTOCOL_HEADER},
    unix_time,
};

const BOOTSTRAP_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientState {
    Idle,
    Connecting,
    Connected,
    Disconnected,
}

struct BootstrapAttempt {
    socket: UdpSocket,
    server_address: SocketAddr,
    handshake: HandshakeState,
    request: Vec<u8>,
    elapsed: Duration,
    since_last_send: Duration,
}

struct ClientConnection {
    renet: RenetClient,
    transport: NetcodeClientTransport,
}

pub struct NetworkClient {
    current_time: Duration,
    state: ClientState,
    client_id: Option<u64>,
    disconnect_reason: Option<DisconnectReason>,
    bootstrap: Option<BootstrapAttempt>,
    connection: Option<ClientConnection>,
}

impl NetworkClient {
    pub fn new() -> Self {
        Self {
            current_time: Duration::ZERO,
            state: ClientState::Idle,
            client_id: None,
            disconnect_reason: None,
            bootstrap: None,
            connection: None,
        }
    }

    pub fn state(&self) -> ClientState {
        self.state
    }

    pub fn client_id(&self) -> Option<u64> {
        self.client_id
    }

    pub fn disconnect_reason(&self) -> Option<DisconnectReason> {
        self.disconnect_reason
    }

    pub fn is_connected(&self) -> bool {
        self.state == ClientState::Connected
    }

    pub fn is_disconnected(&self) -> bool {
        self.state == ClientState::Disconnected
    }

    pub fn connect(&mut self, bootstrap: Bootstrap) -> NetworkResult<()> {
        if !matches!(self.state, ClientState::Idle | ClientState::Disconnected) {
            return Err(NetworkError::InvalidState(
                "client is already connecting or connected",
            ));
        }

        let local_address = match bootstrap.address {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        let socket = UdpSocket::bind(local_address)?;
        socket.set_nonblocking(true)?;
        let mut handshake = Builder::new(noise_params())
            .remote_public_key(&bootstrap.public_key)?
            .build_initiator()?;
        let mut request = [0; BOOTSTRAP_MAX_PACKET_SIZE];
        request[..PROTOCOL_HEADER.len()].copy_from_slice(&PROTOCOL_HEADER);
        let request_size = handshake.write_message(&[], &mut request[PROTOCOL_HEADER.len()..])?
            + PROTOCOL_HEADER.len();
        socket.send_to(&request[..request_size], bootstrap.address)?;

        self.current_time = unix_time()?;
        self.state = ClientState::Connecting;
        self.client_id = None;
        self.disconnect_reason = None;
        self.connection = None;
        self.bootstrap = Some(BootstrapAttempt {
            socket,
            server_address: bootstrap.address,
            handshake,
            request: request[..request_size].to_vec(),
            elapsed: Duration::ZERO,
            since_last_send: Duration::ZERO,
        });
        Ok(())
    }

    pub fn disconnect(&mut self) -> NetworkResult<()> {
        if !matches!(self.state, ClientState::Connecting | ClientState::Connected) {
            return Err(NetworkError::InvalidState(
                "client is not connecting or connected",
            ));
        }

        self.finish_disconnect(DisconnectReason::Requested);
        Ok(())
    }

    pub fn update(&mut self, delta: Duration) -> NetworkResult<()> {
        self.current_time += delta;
        self.update_bootstrap(delta)?;

        let disconnect_reason = {
            let Some(connection) = self.connection.as_mut() else {
                return Ok(());
            };

            connection.renet.update(delta);
            let update_result = connection
                .transport
                .update(delta, &mut connection.renet)
                .and_then(|_| connection.transport.send_packets(&mut connection.renet));

            match update_result {
                Ok(()) => {
                    self.state = if connection.renet.is_connected() {
                        ClientState::Connected
                    } else {
                        ClientState::Connecting
                    };
                    None
                }
                Err(error) => match disconnect_reason_from(&error) {
                    Some(reason) => Some(reason),
                    None => return Err(error.into()),
                },
            }
        };

        if let Some(reason) = disconnect_reason {
            self.finish_disconnect(reason);
        }
        Ok(())
    }

    pub fn send(&mut self, channel: Channel, payload: impl Into<Vec<u8>>) -> NetworkResult<()> {
        let connection = self
            .connection
            .as_mut()
            .filter(|connection| connection.renet.is_connected())
            .ok_or(NetworkError::InvalidState("client is not connected"))?;
        connection
            .renet
            .send_message(renet::DefaultChannel::from(channel), payload.into());
        Ok(())
    }

    pub fn receive(&mut self, channel: Channel) -> Option<Vec<u8>> {
        self.connection
            .as_mut()?
            .renet
            .receive_message(renet::DefaultChannel::from(channel))
            .map(|message| message.to_vec())
    }

    fn update_bootstrap(&mut self, delta: Duration) -> NetworkResult<()> {
        let Some(mut attempt) = self.bootstrap.take() else {
            return Ok(());
        };
        attempt.elapsed += delta;
        attempt.since_last_send += delta;
        let mut response_packet = [0; BOOTSTRAP_MAX_PACKET_SIZE];

        loop {
            let (response_size, server_address) =
                match attempt.socket.recv_from(&mut response_packet) {
                    Ok(received) => received,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        self.finish_disconnect(DisconnectReason::TransportError);
                        return Err(error.into());
                    }
                };

            if server_address != attempt.server_address {
                continue;
            }
            let Some(response) = response_packet[..response_size].strip_prefix(&PROTOCOL_HEADER)
            else {
                continue;
            };

            let mut token_bytes = [0; BOOTSTRAP_MAX_PACKET_SIZE];
            let token_size = match attempt.handshake.read_message(response, &mut token_bytes) {
                Ok(size) => size,
                Err(_) => continue,
            };
            if !attempt.handshake.is_handshake_finished() {
                self.finish_disconnect(DisconnectReason::ProtocolError);
                return Err(NetworkError::InvalidBootstrapResponse);
            }

            let token = match ConnectToken::read(&mut &token_bytes[..token_size]) {
                Ok(token) => token,
                Err(error) => {
                    self.finish_disconnect(DisconnectReason::ProtocolError);
                    return Err(error.into());
                }
            };
            let client_id = token.client_id;
            let transport = match NetcodeClientTransport::new(
                self.current_time,
                ClientAuthentication::Secure {
                    connect_token: token,
                },
                attempt.socket,
            ) {
                Ok(transport) => transport,
                Err(error) => {
                    self.finish_disconnect(DisconnectReason::ProtocolError);
                    return Err(error.into());
                }
            };

            self.client_id = Some(client_id);
            self.connection = Some(ClientConnection {
                renet: RenetClient::new(ConnectionConfig::default()),
                transport,
            });
            return Ok(());
        }

        if attempt.elapsed >= BOOTSTRAP_TIMEOUT {
            self.finish_disconnect(DisconnectReason::BootstrapTimedOut);
            return Ok(());
        }
        if attempt.since_last_send >= BOOTSTRAP_RETRY_INTERVAL {
            if let Err(error) = attempt
                .socket
                .send_to(&attempt.request, attempt.server_address)
            {
                self.finish_disconnect(DisconnectReason::TransportError);
                return Err(error.into());
            }
            attempt.since_last_send = Duration::ZERO;
        }

        self.bootstrap = Some(attempt);
        Ok(())
    }

    fn finish_disconnect(&mut self, reason: DisconnectReason) {
        if let Some(connection) = self.connection.as_mut() {
            connection.transport.disconnect();
        }
        self.bootstrap = None;
        self.connection = None;
        self.client_id = None;
        self.disconnect_reason = Some(reason);
        self.state = ClientState::Disconnected;
    }
}

impl Default for NetworkClient {
    fn default() -> Self {
        Self::new()
    }
}

fn disconnect_reason_from(error: &NetcodeTransportError) -> Option<DisconnectReason> {
    match error {
        NetcodeTransportError::Netcode(NetcodeError::Disconnected(reason)) => {
            Some((*reason).into())
        }
        NetcodeTransportError::Renet(reason) => Some((*reason).into()),
        NetcodeTransportError::Netcode(_) | NetcodeTransportError::IO(_) => None,
    }
}
