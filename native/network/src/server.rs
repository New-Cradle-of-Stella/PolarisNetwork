use std::{
    net::{SocketAddr, UdpSocket},
    time::Duration,
};

use renet::{ConnectionConfig, RenetServer};
use renet_netcode::{
    NETCODE_KEY_BYTES, NetcodeServerTransport, ServerAuthentication, ServerConfig,
    generate_random_bytes,
};

use crate::{
    Bootstrap, Channel, NetworkError, NetworkResult, bootstrap::BootstrapServer,
    protocol::PROTOCOL_ID, unix_time,
};

const MAX_SUPPORTED_CLIENTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerState {
    Idle,
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkServerConfig {
    pub game_address: SocketAddr,
    pub bootstrap_address: SocketAddr,
    pub max_clients: usize,
    pub connection_timeout: Duration,
}

impl NetworkServerConfig {
    fn connection_timeout_seconds(self) -> NetworkResult<i32> {
        if self.game_address.port() == 0 || self.bootstrap_address.port() == 0 {
            return Err(NetworkError::InvalidConfig(
                "server ports must be explicitly configured",
            ));
        }
        if self.game_address == self.bootstrap_address {
            return Err(NetworkError::InvalidConfig(
                "game and bootstrap addresses must differ",
            ));
        }
        if !(1..=MAX_SUPPORTED_CLIENTS).contains(&self.max_clients) {
            return Err(NetworkError::InvalidConfig(
                "max_clients must be between 1 and 1024",
            ));
        }

        let seconds = i32::try_from(self.connection_timeout.as_secs())
            .map_err(|_| NetworkError::InvalidConfig("connection_timeout is too large"))?;
        if seconds == 0 {
            return Err(NetworkError::InvalidConfig(
                "connection_timeout must be at least one second",
            ));
        }
        Ok(seconds)
    }
}

struct ServerConnection {
    renet: RenetServer,
    transport: NetcodeServerTransport,
}

struct RunningServer {
    connection: ServerConnection,
    bootstrap: BootstrapServer,
}

pub struct NetworkServer {
    config: NetworkServerConfig,
    current_time: Duration,
    state: ServerState,
    running: Option<RunningServer>,
}

impl NetworkServer {
    pub fn new(config: NetworkServerConfig) -> NetworkResult<Self> {
        config.connection_timeout_seconds()?;
        Ok(Self {
            config,
            current_time: Duration::ZERO,
            state: ServerState::Idle,
            running: None,
        })
    }

    pub fn state(&self) -> ServerState {
        self.state
    }

    pub fn address(&self) -> NetworkResult<SocketAddr> {
        if self.state != ServerState::Running {
            return Err(NetworkError::InvalidState("server is not running"));
        }
        Ok(self.config.game_address)
    }

    pub fn bootstrap(&self) -> NetworkResult<Bootstrap> {
        Ok(self.running()?.bootstrap.bootstrap())
    }

    pub fn is_connected(&self, client_id: u64) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.connection.renet.is_connected(client_id))
    }

    pub fn start(&mut self) -> NetworkResult<()> {
        if !matches!(self.state, ServerState::Idle | ServerState::Stopped) {
            return Err(NetworkError::InvalidState("server is already running"));
        }

        let current_time = unix_time()?;
        let connection_timeout_seconds = self.config.connection_timeout_seconds()?;
        let game_socket = UdpSocket::bind(self.config.game_address)?;
        let netcode_key: [u8; NETCODE_KEY_BYTES] = generate_random_bytes();
        let server_config = ServerConfig {
            current_time,
            max_clients: self.config.max_clients,
            protocol_id: PROTOCOL_ID,
            public_addresses: vec![self.config.game_address],
            authentication: ServerAuthentication::Secure {
                private_key: netcode_key,
            },
        };
        let connection = ServerConnection {
            renet: RenetServer::new(ConnectionConfig::default()),
            transport: NetcodeServerTransport::new(server_config, game_socket)?,
        };
        let bootstrap = BootstrapServer::bind(
            self.config.bootstrap_address,
            self.config.game_address,
            netcode_key,
            connection_timeout_seconds,
            self.config.max_clients,
        )?;

        self.current_time = current_time;
        self.running = Some(RunningServer {
            connection,
            bootstrap,
        });
        self.state = ServerState::Running;
        Ok(())
    }

    pub fn stop(&mut self) -> NetworkResult<()> {
        let mut running = self
            .running
            .take()
            .ok_or(NetworkError::InvalidState("server is not running"))?;
        running
            .connection
            .transport
            .disconnect_all(&mut running.connection.renet);
        self.state = ServerState::Stopped;
        Ok(())
    }

    pub fn update(&mut self, delta: Duration) -> NetworkResult<()> {
        self.current_time += delta;
        let current_time = self.current_time;
        let running = self.running_mut()?;
        running.bootstrap.receive_requests(current_time)?;
        running.connection.renet.update(delta);
        running
            .connection
            .transport
            .update(delta, &mut running.connection.renet)?;
        running
            .connection
            .transport
            .send_packets(&mut running.connection.renet);
        Ok(())
    }

    pub fn send(
        &mut self,
        client_id: u64,
        channel: Channel,
        payload: impl Into<Vec<u8>>,
    ) -> NetworkResult<()> {
        if !self.is_connected(client_id) {
            return Err(NetworkError::InvalidState("client is not connected"));
        }

        self.running_mut()?.connection.renet.send_message(
            client_id,
            renet::DefaultChannel::from(channel),
            payload.into(),
        );
        Ok(())
    }

    pub fn receive(&mut self, client_id: u64, channel: Channel) -> Option<Vec<u8>> {
        self.running
            .as_mut()?
            .connection
            .renet
            .receive_message(client_id, renet::DefaultChannel::from(channel))
            .map(|message| message.to_vec())
    }

    fn running(&self) -> NetworkResult<&RunningServer> {
        self.running
            .as_ref()
            .ok_or(NetworkError::InvalidState("server is not running"))
    }

    fn running_mut(&mut self) -> NetworkResult<&mut RunningServer> {
        self.running
            .as_mut()
            .ok_or(NetworkError::InvalidState("server is not running"))
    }
}
