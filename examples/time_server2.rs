// Copyright 2017 tokio-jsonrpc Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// TODO: Some comments explaining what is happening

extern crate futures;
#[macro_use]
extern crate serde_json;
extern crate tokio;
extern crate tokio_jsonrpc;

use futures::{Future, Sink, Stream, lazy};
use tokio::prelude::*;
use tokio::net::TcpListener;
use tokio::io::AsyncRead;
use tokio::codec::Framed;

use tokio_jsonrpc::{BoundaryCodec, Message, RpcError, LineCodec};
use tokio_jsonrpc::codec::DirtyLine;
use tokio_jsonrpc::message::Notification;
use serde_json::Value;

//fn handle(message: Message) -> impl Future<Item=Value, Error=RpcError> {
//
//}

fn main() {
    let addr = "127.0.0.1:3030".parse().unwrap();

    let listener = TcpListener::bind(&addr)
        .expect("Unable to bind TCP listener");
    let incoming = listener.incoming();

    let server = incoming
        .map_err(|e| eprintln!("accept failed = {:?}", e))
        .for_each(|socket| {
            let jsonized = Framed::new(socket, DirtyLine::new());
            let (w, r) = jsonized.split();
            let action = r
                .map(|message| {
                    Message::Batch(vec![])
                })
                .forward(w)
                .map(|_| ())
                .map_err(|err| {
                    println!("error")
                });
            tokio::spawn(action);
            Ok(())
        });
    tokio::run(server);
//    let service = connections.for_each(|(stream, _)| {
//        let jsonized = stream.framed(LineCodec::new());
//        let (w, r) = jsonized.split();
//        let answers = r.filter_map(|message| {
//            println!("A message received: {:?}", message);
//            match message {
//                Ok(Message::Request(ref req)) => {
//                    println!("Got method {}", req.method);
//                    if req.method == "echo" {
//                        Some(req.reply(json!([req.method, req.params])))
//                    } else {
//                        Some(req.error(RpcError::method_not_found(req.method.clone())))
//                    }
//                },
//                Ok(Message::Notification(Notification { ref method, .. })) => {
//                    println!("Got notification {}", method);
//                    None
//                },
//                Err(ref e) => Some(e.reply()),
//                _ => None,
//            }
//        });
//        let sent = w.send_all(answers)
//            .map(|_| ())
//            .map_err(|e| println!("{}", e));
//        // Do the sending in the background
//        handle.spawn(sent);
//        Ok(())
//    });
//    core.run(service).unwrap();
}
