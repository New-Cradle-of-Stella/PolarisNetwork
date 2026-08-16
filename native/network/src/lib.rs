#[cfg(test)]
mod tests {
    use renet::{ClientId, ConnectionConfig, DefaultChannel, RenetServer};

    #[test]
    fn reliable_ordered_ping() {
        let client_id: ClientId = 1;
        let mut server = RenetServer::new(ConnectionConfig::default());
        let mut client = server.new_local_client(client_id);

        client.send_message(DefaultChannel::ReliableOrdered, b"ping".to_vec());
        server.process_local_client(client_id, &mut client).unwrap();

        let ping = server
            .receive_message(client_id, DefaultChannel::ReliableOrdered)
            .expect("server should receive ping");

        assert_eq!(ping.as_ref(), b"ping");

        server.send_message(client_id, DefaultChannel::ReliableOrdered, b"pong".to_vec());
        server.process_local_client(client_id, &mut client).unwrap();

        let pong = client
            .receive_message(DefaultChannel::ReliableOrdered)
            .expect("client should receive pong");

        assert_eq!(pong.as_ref(), b"pong");
    }
}
