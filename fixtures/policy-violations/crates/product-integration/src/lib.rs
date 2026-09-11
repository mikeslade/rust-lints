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
        let _contents = std::fs::read_to_string("synthetic-fixture.txt"); // blocking-in-async: expect
    };
}

// VIOLATION: synchronous socket I/O on an async worker thread.
pub fn blocking_socket_inside_async_context(addr: std::net::SocketAddr) {
    let _future = async move {
        let _stream = std::net::TcpStream::connect(addr); // blocking-in-async: expect
    };
}

// VIOLATION: `ToSocketAddrs` runs the blocking OS resolver.
pub fn blocking_dns_inside_async_context(host: String) {
    let _future = async move {
        use std::net::ToSocketAddrs;
        let _addrs = host.to_socket_addrs(); // blocking-in-async: expect
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

// The closed-trait cases. Each is an implementation of the same trait, spelled a
// different way; the pass sees one `DefId` for all of them. Every line that must
// be flagged carries a trailing closed-trait-impl expect marker, and
// scripts/check-closed-trait-impl-fixture.sh set-diffs those markers against
// what the pass reports, in both directions.

use fixture_product_core::OutboxRepository;
use fixture_product_core::OutboxRepository as AliasedRepository;
use fixture_product_core::RenderedPayload;

pub struct GenericWriter<T> {
    pub source: T,
}

// VIOLATION: nested angle brackets in the generic bound. The regex inventory
// this pass replaced could not span them and reported nothing here.
impl<T: Iterator<Item = Vec<u8>> + Send + Sync> OutboxRepository for GenericWriter<T> { // closed-trait-impl: expect
    fn store(&self, _payload: &RenderedPayload) {}
}

pub struct AliasedWriter;

// VIOLATION: the trait is named through an import alias, so no textual search
// for the trait's own name finds this impl.
impl AliasedRepository for AliasedWriter { // closed-trait-impl: expect
    fn store(&self, _payload: &RenderedPayload) {}
}

macro_rules! outbox_writer {
    ($name:ident) => {
        pub struct $name;

        // VIOLATION: the impl header exists only after expansion.
        impl OutboxRepository for $name { // closed-trait-impl: expect
            fn store(&self, _payload: &RenderedPayload) {}
        }
    };
}

outbox_writer!(MacroWriter);

pub struct ReviewedWriter;

// COMPLIANT: named in the allowed set the fixture script passes, so it carries
// no marker and must not be reported. Removing the allowed-set filter turns this
// line into a false positive, which is how the script catches a pass that has
// widened into "flag every impl".
impl OutboxRepository for ReviewedWriter {
    fn store(&self, _payload: &RenderedPayload) {}
}
