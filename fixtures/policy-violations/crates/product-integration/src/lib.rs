mod reqwest {
    pub struct Client;
    pub struct Builder;

    impl Client {
        pub fn builder() -> Builder {
            Builder
        }
    }
}

pub fn direct_http_client() {
    let _client = reqwest::Client::builder();
}

pub fn blocking_call_inside_async_context() {
    let _future = async {
        let _contents = std::fs::read_to_string("synthetic-fixture.txt");
    };
}

// VIOLATION: synchronous socket I/O on an async worker thread.
pub fn blocking_socket_inside_async_context(addr: std::net::SocketAddr) {
    let _future = async move {
        let _stream = std::net::TcpStream::connect(addr);
    };
}

// VIOLATION: `ToSocketAddrs` runs the blocking OS resolver.
pub fn blocking_dns_inside_async_context(host: String) {
    let _future = async move {
        use std::net::ToSocketAddrs;
        let _addrs = host.to_socket_addrs();
    };
}

// COMPLIANT: `SocketAddr` is an address value, not a socket. Its accessors read
// in-memory bytes and block nothing, so they must not be flagged even though
// they live under `std::net`.
pub fn address_value_accessors_inside_async_context(addr: std::net::SocketAddr) {
    let _future = async move {
        let _port = addr.port();
        let _ip = addr.ip();
        let _v4 = addr.is_ipv4();
    };
}
