extern crate futures;
extern crate grpc;
extern crate tokio_core;
extern crate tokio_tls_api;
#[macro_use]
extern crate log;
extern crate log_ndc_env_logger;

mod test_misc;

use std::sync::Arc;

use futures::future::*;
use futures::stream::Stream;

use grpc::rt::*;
use grpc::*;

use test_misc::*;
use std::thread;

/// Single method server on random port
fn new_server<H>(service: &str, method: &str, handler: H) -> Server
where
    H: MethodHandler<String, String> + 'static + Sync + Send,
    H: GrpcStreamingFlavor,
{
    let mut methods = Vec::new();
    methods.push(ServerMethod::new(
        string_string_method(
            &format!("{}{}", service, method),
            <H as GrpcStreamingFlavor>::streaming(),
        ),
        handler,
    ));
    let mut server = ServerBuilder::new_plain();
    server.http.set_port(0);
    server.add_service(ServerServiceDefinition::new(service, methods));
    server.build().expect("server")
}

/// Single unary method server
fn new_server_unary<H>(service: &str, method: &str, handler: H) -> Server
where
    H: Fn(ServerHandlerContext, ServerRequestSingle<String>, ServerResponseUnarySink<String>) -> grpc::Result<()> + Sync + Send + 'static,
{
    new_server(service, method, MethodHandlerUnary::new(handler))
}

/// Single server streaming method server
fn new_server_server_streaming<H>(service: &str, method: &str, handler: H) -> Server
where
    H: Fn(ServerHandlerContext, ServerRequestSingle<String>, ServerResponseSink<String>) -> grpc::Result<()> + Sync + Send + 'static,
{
    new_server(service, method, MethodHandlerServerStreaming::new(handler))
}

/// Single server streaming method server
fn new_server_client_streaming<H>(service: &str, method: &str, handler: H) -> Server
where
    H: Fn(ServerHandlerContext, ServerRequest<String>, ServerResponseUnarySink<String>) -> grpc::Result<()>
        + Sync
        + Send
        + 'static,
{
    new_server(service, method, MethodHandlerClientStreaming::new(handler))
}

/// Tester for unary methods
struct TesterUnary {
    name: String,
    client: Client,
    _server: Server,
}

impl TesterUnary {
    fn new<H>(handler: H) -> TesterUnary
    where
        H: Fn(ServerHandlerContext, ServerRequestSingle<String>, ServerResponseUnarySink<String>) -> grpc::Result<()> + Sync + Send + 'static,
    {
        let server = new_server_unary("/text", "/Unary", handler);
        let port = server.local_addr().port().expect("port");
        TesterUnary {
            name: "/text/Unary".to_owned(),
            _server: server,
            client: Client::new_plain(BIND_HOST, port, Default::default()).unwrap(),
        }
    }

    fn call(&self, param: &str) -> GrpcFuture<String> {
        self.client
            .call_unary(
                RequestOptions::new(),
                param.to_owned(),
                string_string_method(&self.name, GrpcStreaming::Unary),
            ).drop_metadata()
    }

    fn call_expect_error<F: FnOnce(&Error) -> bool>(&self, param: &str, expect: F) {
        match self.call(param).wait() {
            Ok(r) => panic!("expecting error, got: {:?}", r),
            Err(e) => assert!(expect(&e), "wrong error: {:?}", e),
        }
    }

    fn call_expect_grpc_error<F: FnOnce(&str) -> bool>(&self, param: &str, expect: F) {
        self.call_expect_error(param, |e| match e {
            &Error::GrpcMessage(GrpcMessageError {
                ref grpc_message, ..
            })
                if expect(&grpc_message) =>
            {
                true
            }
            _ => false,
        });
    }

    fn call_expect_grpc_error_contain(&self, param: &str, expect: &str) {
        self.call_expect_grpc_error(param, |m| m.find(expect).is_some());
    }
}

/// Tester for server streaming methods
struct TesterServerStreaming {
    name: String,
    client: Client,
    _server: Server,
}

impl TesterServerStreaming {
    fn new<H>(handler: H) -> Self
    where
        H: Fn(ServerHandlerContext, ServerRequestSingle<String>, ServerResponseSink<String>) -> grpc::Result<()> + Sync + Send + 'static,
    {
        let server = new_server_server_streaming("/test", "/ServerStreaming", handler);
        let port = server.local_addr().port().expect("port");
        TesterServerStreaming {
            name: "/test/ServerStreaming".to_owned(),
            _server: server,
            client: Client::new_plain(BIND_HOST, port, Default::default()).unwrap(),
        }
    }

    fn call(&self, param: &str) -> GrpcStream<String> {
        self.client
            .call_server_streaming(
                RequestOptions::new(),
                param.to_owned(),
                string_string_method(&self.name, GrpcStreaming::ServerStreaming),
            ).drop_metadata()
    }
}

struct TesterClientStreaming {
    name: String,
    client: Client,
    _server: Server,
}

impl TesterClientStreaming {
    fn new<H>(handler: H) -> Self
    where
        H: Fn(ServerHandlerContext, ServerRequest<String>, ServerResponseUnarySink<String>) -> grpc::Result<()>
            + Sync
            + Send
            + 'static,
    {
        let server = new_server_client_streaming("/test", "/ClientStreaming", handler);
        let port = server.local_addr().port().expect("port");
        TesterClientStreaming {
            name: "/test/ClientStreaming".to_owned(),
            _server: server,
            client: Client::new_plain(BIND_HOST, port, Default::default()).unwrap(),
        }
    }

    fn call(&self) -> (ClientRequestSink<String>, SingleResponse<String>) {
        self.client
                .call_client_streaming(
                    RequestOptions::new(),
                    string_string_method(&self.name, GrpcStreaming::ClientStreaming),
                )
                .wait().unwrap()
    }
}

#[test]
fn unary() {
    init_logger();

    let tester = TesterUnary::new(|_m, req, resp| resp.finish(req.message));

    assert_eq!("aa", tester.call("aa").wait().unwrap());
}

#[test]
fn error_in_handler() {
    init_logger();

    let tester = TesterUnary::new(|_m, _req, _resp| {
        Err(grpc::Error::Other("my error"))
    });

    tester.call_expect_grpc_error_contain("aa", "grpc server handler did not close the sender");
}

// TODO
//#[test]
fn _panic_in_handler() {
    init_logger();

    let tester = TesterUnary::new(|_m, _req, _resp| panic!("icnap"));

    tester.call_expect_grpc_error("aa", |m| {
        m.find("Panic").is_some() && m.find("icnap").is_some()
    });
}

#[test]
fn server_streaming() {
    init_logger();

    let test_sync = Arc::new(TestSync::new());
    let test_sync_server = test_sync.clone();

    let tester = TesterServerStreaming::new(move |_m, req, mut resp| {
        let sync = test_sync_server.clone();
        thread::spawn(move || {
            for i in 0..3 {
                sync.take(i * 2 + 1);
                resp.send_data(format!("{}{}", req.message, i)).unwrap();
            }
            resp.send_trailers(Metadata::new()).unwrap();
        });
        Ok(())
    });

    let mut rs = tester.call("x").wait();

    test_sync.take(0);
    assert_eq!("x0", rs.next().unwrap().unwrap());
    test_sync.take(2);
    assert_eq!("x1", rs.next().unwrap().unwrap());
    test_sync.take(4);
    assert_eq!("x2", rs.next().unwrap().unwrap());
    assert!(rs.next().is_none());
}

#[test]
fn client_streaming() {
    init_logger();

    let tester = TesterClientStreaming::new(move |m, req, resp| {
        let request_stream = req.into_stream();
        m.ctx.loop_remote().spawn(move |_handle| {
            request_stream.fold(String::new(), |mut s, message| {
                s.push_str(&message);
                futures::finished::<_, Error>(s)
            }).map(|r| {
                resp.finish(r).unwrap();
            }).map_err(|_| ())
        });
        Ok(())
    });

    let (mut tx, result) = tester.call();
    tx.send_data("aa".to_owned()).expect("aa");
    tx.send_data("bb".to_owned()).expect("bb");
    tx.send_data("cc".to_owned()).expect("cc");
    tx.finish().expect("finish");

    assert_eq!("aabbcc", result.wait().unwrap().1);
}
