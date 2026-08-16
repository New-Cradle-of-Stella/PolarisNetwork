use std::{
    alloc::System, net::UdpSocket, thread, time::{Duration, SystemTime, UNIX_EPOCH},
};
use renet::{ClientId, ConnectionConfig, RenetClient, RenetServer};
use renet_netcode::{
    ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication,
    ServerConfig,
};

#[test]
fn udp_client_connect_to_server() {
    const PROTOCOL_ID: u64 = 7;
    const CLEINT_ID: ClientId = 1;

    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    
    let server_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_socket.local_addr().unwrap();
    let server_config = ServerConfig {
        current_time,
        max_clients: 1,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![server_addr],
        authentication: ServerAuthentication::Unsecure,
    };
    let mut server_transport = NetcodeServerTransport::new(server_config, server_socket);

    

    
}