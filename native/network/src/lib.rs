mod bootstrap;
mod client;
mod protocol;
mod server;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use renet::{DefaultChannel, DisconnectReason as RenetDisconnectReason};
use renet_netcode::{
    NetcodeDisconnectReason, NetcodeError, NetcodeTransportError, TokenGenerationError,
};
use thiserror::Error;

pub use bootstrap::Bootstrap;
pub use client::{ClientState, NetworkClient};
pub use server::{NetworkServer, NetworkServerConfig, ServerState};

pub type NetworkResult<T> = Result<T, NetworkError>;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Noise(#[from] snow::Error),
    #[error(transparent)]
    Netcode(#[from] NetcodeError),
    #[error(transparent)]
    Transport(#[from] NetcodeTransportError),
    #[error(transparent)]
    Token(#[from] TokenGenerationError),
    #[error("{0}")]
    InvalidConfig(&'static str),
    #[error("{0}")]
    InvalidState(&'static str),
    #[error("client id space exhausted")]
    ClientIdExhausted,
    #[error("invalid bootstrap response")]
    InvalidBootstrapResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    Requested,
    BootstrapTimedOut,
    ConnectTokenExpired,
    ConnectionTimedOut,
    ConnectionResponseTimedOut,
    ConnectionRequestTimedOut,
    ConnectionDenied,
    DisconnectedByServer,
    TransportError,
    ProtocolError,
}

impl From<NetcodeDisconnectReason> for DisconnectReason {
    fn from(reason: NetcodeDisconnectReason) -> Self {
        match reason {
            NetcodeDisconnectReason::ConnectTokenExpired => Self::ConnectTokenExpired,
            NetcodeDisconnectReason::ConnectionTimedOut => Self::ConnectionTimedOut,
            NetcodeDisconnectReason::ConnectionResponseTimedOut => Self::ConnectionResponseTimedOut,
            NetcodeDisconnectReason::ConnectionRequestTimedOut => Self::ConnectionRequestTimedOut,
            NetcodeDisconnectReason::ConnectionDenied => Self::ConnectionDenied,
            NetcodeDisconnectReason::DisconnectedByClient => Self::Requested,
            NetcodeDisconnectReason::DisconnectedByServer => Self::DisconnectedByServer,
        }
    }
}

impl From<RenetDisconnectReason> for DisconnectReason {
    fn from(reason: RenetDisconnectReason) -> Self {
        match reason {
            RenetDisconnectReason::Transport => Self::TransportError,
            RenetDisconnectReason::DisconnectedByClient => Self::Requested,
            RenetDisconnectReason::DisconnectedByServer => Self::DisconnectedByServer,
            RenetDisconnectReason::PacketSerialization(_)
            | RenetDisconnectReason::PacketDeserialization(_)
            | RenetDisconnectReason::ReceivedInvalidChannelId(_)
            | RenetDisconnectReason::SendChannelError { .. }
            | RenetDisconnectReason::ReceiveChannelError { .. } => Self::ProtocolError,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Unreliable,
    ReliableUnordered,
    ReliableOrdered,
}

impl From<Channel> for DefaultChannel {
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Unreliable => Self::Unreliable,
            Channel::ReliableUnordered => Self::ReliableUnordered,
            Channel::ReliableOrdered => Self::ReliableOrdered,
        }
    }
}

pub(crate) fn unix_time() -> std::io::Result<Duration> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)
}
