mod connection {
    use std::{
        io,
        net::{Ipv4Addr, SocketAddr, UdpSocket},
        ptr,
        time::Duration,
    };

    use polaris_network_native::{
        Channel, ClientState, DisconnectReason, NetworkClient, NetworkServer, NetworkServerConfig,
        ServerState,
    };

    const STEP: Duration = Duration::from_millis(16);
    const MAX_STEPS: usize = 256;

    fn server_config(game_port: u16, bootstrap_port: u16) -> NetworkServerConfig {
        NetworkServerConfig {
            game_address: SocketAddr::from((Ipv4Addr::LOCALHOST, game_port)),
            bootstrap_address: SocketAddr::from((Ipv4Addr::LOCALHOST, bootstrap_port)),
            max_clients: 8,
            connection_timeout: Duration::from_secs(5),
        }
    }

    fn drive_until<T>(
        server: &mut NetworkServer,
        client: &mut NetworkClient,
        mut check: impl FnMut(&mut NetworkServer, &mut NetworkClient) -> Option<T>,
    ) -> T {
        for _ in 0..MAX_STEPS {
            client.update(STEP).unwrap();
            server.update(STEP).unwrap();

            if let Some(value) = check(server, client) {
                return value;
            }
        }

        panic!("network operation did not finish within {MAX_STEPS} steps");
    }

    fn connected_pair(game_port: u16, bootstrap_port: u16) -> (NetworkServer, NetworkClient, u64) {
        let mut server = NetworkServer::new(server_config(game_port, bootstrap_port)).unwrap();
        assert_eq!(server.state(), ServerState::Idle);
        server.start().unwrap();

        let mut client = NetworkClient::new();
        client.connect(server.bootstrap().unwrap()).unwrap();
        let client_id = drive_until(&mut server, &mut client, |server, client| {
            let client_id = client.client_id()?;
            (client.is_connected() && server.is_connected(client_id)).then_some(client_id)
        });

        (server, client, client_id)
    }

    #[test]
    fn connect() {
        let (server, client, client_id) = connected_pair(43000, 43001);

        assert_eq!(server.state(), ServerState::Running);
        assert_eq!(client.state(), ClientState::Connected);
        assert!(server.is_connected(client_id));
    }

    #[test]
    fn ping() {
        let (mut server, mut client, client_id) = connected_pair(43002, 43003);

        client.send(Channel::ReliableOrdered, b"ping").unwrap();
        let ping = drive_until(&mut server, &mut client, |server, _| {
            server.receive(client_id, Channel::ReliableOrdered)
        });
        assert_eq!(ping.as_slice(), b"ping");

        server
            .send(client_id, Channel::ReliableOrdered, b"pong")
            .unwrap();
        let pong = drive_until(&mut server, &mut client, |_, client| {
            client.receive(Channel::ReliableOrdered)
        });
        assert_eq!(pong.as_slice(), b"pong");
    }

    #[test]
    fn reuse_client() {
        let (mut server, mut client, old_client_id) = connected_pair(43004, 43005);

        let bootstrap = server.bootstrap().unwrap();
        let original_client = ptr::from_ref(&client);

        client.disconnect().unwrap();
        assert_eq!(
            client.disconnect_reason(),
            Some(DisconnectReason::Requested)
        );
        drive_until(&mut server, &mut client, |server, client| {
            (client.is_disconnected() && !server.is_connected(old_client_id)).then_some(())
        });

        client.connect(bootstrap).unwrap();
        assert_eq!(client.disconnect_reason(), None);
        let new_client_id = drive_until(&mut server, &mut client, |server, client| {
            let client_id = client.client_id()?;
            (client.is_connected() && server.is_connected(client_id)).then_some(client_id)
        });

        assert_eq!(ptr::from_ref(&client), original_client);
        assert_ne!(new_client_id, old_client_id);
        assert_eq!(client.state(), ClientState::Connected);
        assert!(server.is_connected(new_client_id));
    }

    #[test]
    fn bootstrap_retry() {
        let mut server = NetworkServer::new(server_config(43006, 43007)).unwrap();
        server.start().unwrap();
        let mut bootstrap = server.bootstrap().unwrap();
        let proxy_address = SocketAddr::from((Ipv4Addr::LOCALHOST, 43008));
        let mut proxy = DropFirstResponseProxy::bind(proxy_address, bootstrap.address);
        bootstrap.address = proxy_address;

        let mut client = NetworkClient::new();
        client.connect(bootstrap).unwrap();

        let client_id = drive_through_proxy(&mut server, &mut client, &mut proxy);
        assert!(proxy.dropped_response);
        assert_eq!(client_id, 1);
    }

    struct DropFirstResponseProxy {
        socket: UdpSocket,
        server_address: SocketAddr,
        client_address: Option<SocketAddr>,
        dropped_response: bool,
    }

    impl DropFirstResponseProxy {
        fn bind(address: SocketAddr, server_address: SocketAddr) -> Self {
            let socket = UdpSocket::bind(address).unwrap();
            socket.set_nonblocking(true).unwrap();
            Self {
                socket,
                server_address,
                client_address: None,
                dropped_response: false,
            }
        }

        fn update(&mut self) {
            let mut packet = [0; 1500];
            loop {
                let (size, source) = match self.socket.recv_from(&mut packet) {
                    Ok(received) => received,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                    Err(error) => panic!("proxy receive failed: {error}"),
                };

                if source == self.server_address {
                    if !self.dropped_response {
                        self.dropped_response = true;
                        continue;
                    }
                    self.socket
                        .send_to(&packet[..size], self.client_address.unwrap())
                        .unwrap();
                } else {
                    self.client_address = Some(source);
                    self.socket
                        .send_to(&packet[..size], self.server_address)
                        .unwrap();
                }
            }
        }
    }

    fn drive_through_proxy(
        server: &mut NetworkServer,
        client: &mut NetworkClient,
        proxy: &mut DropFirstResponseProxy,
    ) -> u64 {
        for _ in 0..MAX_STEPS {
            client.update(STEP).unwrap();
            proxy.update();
            server.update(STEP).unwrap();
            proxy.update();

            if let Some(client_id) = client.client_id()
                && client.is_connected()
                && server.is_connected(client_id)
            {
                return client_id;
            }
        }

        panic!("client did not connect through the lossy proxy");
    }
}
