pub(crate) const CONNECT_TOKEN_LIFETIME_SECONDS: u64 = 30;
pub(crate) const MAX_BOOTSTRAP_PACKET_SIZE: usize = 1400;
pub(crate) const NOISE_PATTERN: &str = "Noise_NK_25519_ChaChaPoly_SHA256";
pub(crate) const PROTOCOL_ID: u64 = u64::from_be_bytes(*b"a&binc.\0");
pub(crate) const PROTOCOL_HEADER: [u8; size_of::<u64>()] = PROTOCOL_ID.to_be_bytes();
