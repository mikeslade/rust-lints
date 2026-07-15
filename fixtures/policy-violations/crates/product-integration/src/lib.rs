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
