// Copyright 2017 tokio-jsonrpc Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! The [`Server`](trait.Server.html) trait and helpers.
//!
//! The `Server` trait for the use by the [`Endpoint`](../endpoint/struct.Endpoint.html) is defined
//! here. Furthermore, some helpers for convenient creation and composition of servers are
//! available. Note that not all of these helpers are necessarily zero-cost, at least at this time.

use futures::{Future, IntoFuture};
use serde::Serialize;
use serde_json::{Value, to_value};

use endpoint::ServerCtl;
use message::RpcError;

/// The server endpoint.
///
/// This is usually implemented by the end application and provides the actual functionality of the
/// RPC server. It allows composition of more servers together.
///
/// The default implementations of the callbacks return None, indicating that the given method is
/// not known. It allows implementing only RPCs or only notifications without having to worry about
/// the other callback. If you want a server that does nothing at all, use
/// [`Empty`](struct.Empty.html).
pub trait Server {
    /// The successfull result of the RPC call.
    type Success: Serialize;
    /// The result of the RPC call
    ///
    /// Once the future resolves, the value or error is sent to the client as the reply. The reply
    /// is wrapped automatically.
    type RpcCallResult: IntoFuture<Item = Self::Success, Error = RpcError> + 'static;
    /// The result of the RPC call.
    ///
    /// As the client doesn't expect anything in return, both the success and error results are
    /// thrown away and therefore (). However, it still makes sense to distinguish success and
    /// error.
    type NotificationResult: IntoFuture<Item = (), Error = ()> + 'static;
    /// Called when the client requests something.
    ///
    /// This is a callback from the [endpoint](struct.Endpoint.html) when the client requests
    /// something. If the method is unknown, it shall return `None`. This allows composition of
    /// servers.
    ///
    /// Conversion of parameters and handling of errors is up to the implementer of this trait.
    fn rpc(&self, _ctl: &ServerCtl, _method: &str, _params: &Option<Value>)
           -> Option<Self::RpcCallResult> {
        None
    }
    /// Called when the client sends a notification.
    ///
    /// This is a callback from the [endpoint](struct.Endpoint.html) when the client requests
    /// something. If the method is unknown, it shall return `None`. This allows composition of
    /// servers.
    ///
    /// Conversion of parameters and handling of errors is up to the implementer of this trait.
    fn notification(&self, _ctl: &ServerCtl, _method: &str, _params: &Option<Value>)
                    -> Option<Self::NotificationResult> {
        None
    }
    /// Called when the endpoint is initialized.
    ///
    /// It provides a default empty implementation, which can be overriden to hook onto the
    /// initialization.
    fn initialized(&self, _ctl: &ServerCtl) {}
}

/// A RPC server that knows no methods.
///
/// You can use this if you want to have a client-only [Endpoint](struct.Endpoint.html). It simply
/// terminates the server part right away. Or, more conveniently, use `Endpoint`'s
/// [`client_only`](struct.Endpoint.html#method.client_only) method.
pub struct Empty;

impl Server for Empty {
    type Success = ();
    type RpcCallResult = Result<(), RpcError>;
    type NotificationResult = Result<(), ()>;
    fn initialized(&self, ctl: &ServerCtl) {
        ctl.terminate();
    }
}

/// An RPC server wrapper with dynamic dispatch.
///
/// This server wraps another server and converts it into a common ground, so multiple different
/// servers can be used as trait objects. Basically, it boxes the futures it returns and converts
/// the result into `serde_json::Value`. It can then be used together with
/// [`ServerChain`](struct.ServerChain.html) easilly. Note that this conversion incurs
/// runtime costs.
pub struct AbstractServer<S: Server>(S);

impl<S: Server> AbstractServer<S> {
    /// Wraps another server into an abstract server.
    pub fn new(server: S) -> Self {
        AbstractServer(server)
    }
    /// Unwraps the abstract server and provides the one inside back.
    pub fn into_inner(self) -> S {
        self.0
    }
}

/// A RPC call result wrapping trait objects.
pub type BoxRpcCallResult = Box<Future<Item = Value, Error = RpcError>>;
/// A notification call result wrapping trait objects.
pub type BoxNotificationResult = Box<Future<Item = (), Error = ()>>;

impl<S: Server> Server for AbstractServer<S> {
    type Success = Value;
    type RpcCallResult = BoxRpcCallResult;
    type NotificationResult = BoxNotificationResult;
    fn rpc(&self, ctl: &ServerCtl, method: &str, params: &Option<Value>)
           -> Option<Self::RpcCallResult> {
        self.0
            .rpc(ctl, method, params)
            .map(|f| -> Box<Future<Item = Value, Error = RpcError>> {
                let future = f.into_future()
                    .map(|result| {
                        to_value(result)
                            .expect("Your result type is not convertible to JSON, which is a bug")
                    });
                Box::new(future)
            })
    }
    fn notification(&self, ctl: &ServerCtl, method: &str, params: &Option<Value>)
                    -> Option<Self::NotificationResult> {
        // It seems the type signature is computed from inside the closure and it doesn't fit on
        // the outside, so we need to declare it manually :-(
        self.0
            .notification(ctl, method, params)
            .map(|f| -> Box<Future<Item = (), Error = ()>> { Box::new(f.into_future()) })
    }
    fn initialized(&self, ctl: &ServerCtl) {
        self.0.initialized(ctl)
    }
}

/// A type to store servers as trait objects.
///
/// See also [`AbstractServer`](struct.AbstractServer.html) and
/// [`ServerChain`](struct.ServerChain.html).
pub type BoxServer = Box<Server<Success = Value,
                                RpcCallResult = Box<Future<Item = Value, Error = RpcError>>,
                                NotificationResult = Box<Future<Item = (), Error = ()>>>>;

/// A server that chains several other servers.
///
/// This composes multiple servers into one. When a notification or an rpc comes, it tries one by
/// one and passes the call to each of them. If the server provides an answer, the iteration is
/// stopped and that answer is returned. If the server refuses the given method name, another
/// server in the chain is tried, until one is found or we run out of servers.
///
/// Initialization is called on all the servers.
///
/// The [`AbstractServer`](struct.AbstractServer.html) is one of the ways to plug servers with
/// incompatible future and success types inside.
pub struct ServerChain(Vec<BoxServer>);

impl ServerChain {
    /// Construct a new server.
    pub fn new(subservers: Vec<BoxServer>) -> Self {
        ServerChain(subservers)
    }
    /// Consume the server and return the subservers inside.
    pub fn into_inner(self) -> Vec<BoxServer> {
        self.0
    }
    /// Iterate through the servers and return the first result that is `Some(_)`.
    fn iter_chain<R, F: Fn(&BoxServer) -> Option<R>>(&self, f: F) -> Option<R> {
        for sub in &self.0 {
            let result = f(sub);
            if result.is_some() {
                return result;
            }
        }
        None
    }
}

impl Server for ServerChain {
    type Success = Value;
    type RpcCallResult = BoxRpcCallResult;
    type NotificationResult = BoxNotificationResult;
    fn rpc(&self, ctl: &ServerCtl, method: &str, params: &Option<Value>)
           -> Option<Self::RpcCallResult> {
        self.iter_chain(|sub| sub.rpc(ctl, method, params))
    }
    fn notification(&self, ctl: &ServerCtl, method: &str, params: &Option<Value>)
                    -> Option<Self::NotificationResult> {
        self.iter_chain(|sub| sub.notification(ctl, method, params))
    }
    fn initialized(&self, ctl: &ServerCtl) {
        for sub in &self.0 {
            sub.initialized(ctl);
        }
    }
}

macro_rules! jsonrpc_params {
    // When the user asks for no params to be present. In that case we allow no params or null or
    // empty array or dictionary, for better compatibility. This is probably more benevolent than
    // the spec allows.
    ( $value:expr, ) => {
        match *$value {
            // Accept the empty values
            None |
            Some($crate::macro_exports::Value::Null) => (),
            Some($crate::macro_exports::Value::Array(ref arr)) if arr.len() == 0 => (),
            Some($crate::macro_exports::Value::Object(ref obj)) if obj.len() == 0 => (),
            // If it's anything else, complain
            _ => {
                return Err($crate::message::RpcError::
                           invalid_params(Some("Expected no params".to_owned())));
            },
        }
    };
    // An internal helper to decode a single variable and provide a Result instead of returning
    // from the function.
    ( $value:expr, single $varname:ident : $vartype:ty ) => {{
        // Fix the type
        let val: &$crate::macro_exports::Value = $value;
        $crate::macro_exports::from_value::<$vartype>(val.clone()).map_err(|e| {
            $crate::message::RpcError::invalid_params(Some(format!("Incompatible type: {}", e)))
        })
    }};
    // A helper to count number of arguments
    ( arity $head:ident ) => { 1 };
    ( arity $head:ident, $( $tail:ident ),* ) => { 1 + jsonrpc_params!(arity $( $tail ),*) };
    // A helper to recurse on decoding of positional arguments
    ( $spl:expr, accum ( $( $result:tt )* ), positional_decode $vname:ident : $vtype:ty ) => {
        ( $( $result )*
            {
                let spl: &[$crate::macro_exports::Value] = $spl;
                jsonrpc_params!(&spl[0], single $vname: $vtype)?
            },
        )
    };
    ( $spl:expr, accum ( $( $result:tt )* ),
      positional_decode $hname:ident : $htype:ty, $( $tname:ident : $ttype:ty ),+ ) => {{
        let spl: &[$crate::macro_exports::Value] = $spl;
        jsonrpc_params!(&spl[1..], accum (
            $( $result )*
            {
                jsonrpc_params!(&spl[0], single $hname: $htype)?
            },
        ), positional_decode $( $tname: $ttype ),+ )
    }};
    // Possibly multiple arguments, enforcing positional coding (in an array)
    // It uses recursion to count and access the items in the vector
    ( $value:expr, positional $( $varname:ident : $vartype:ty ),+ ) => {{
        let val: &Option<$crate::macro_exports::Value> = $value;
        match *val {
            None => return Err($crate::message::RpcError::
                               invalid_params(Some("Expected parameters".to_owned()))),
            Some(Value::Array(ref vec)) => {
                let cnt = jsonrpc_params!(arity $( $varname ),+);
                if cnt != vec.len() {
                    let err = format!("Wrong number of parameters: expected: {}, got: {}", cnt,
                                      vec.len());
                    return Err($crate::message::RpcError::invalid_params(Some(err)));
                }
                let spl: &[$crate::macro_exports::Value] = &vec[..];
                jsonrpc_params!(spl, accum (), positional_decode $( $varname: $vartype ),+)
            },
            Some(_) => {
                return Err($crate::message::RpcError::
                           invalid_params(Some("Expected an array as parameters".to_owned())));
            },
        }
    }};
    // Decode named arguments.
    // It can handle optional arguments in a way, but it has its limits (eg. a non-optional string
    // defaults to an empty one if it is missing).
    ( $value:expr, named $( $varname:ident : $vartype:ty ),+ ) => {{
        let val: &Option<$crate::macro_exports::Value> = $value;
        match *val {
            None => return Err($crate::message::RpcError::
                               invalid_params(Some("Expected parameters".to_owned()))),
            Some(Value::Object(ref map)) => {
                (
                    $(
                        {
                            // Yes, stupid borrow checker… can't we get a global constant that
                            // never gets dropped?
                            let null = Value::Null;
                            let subval = map.get(stringify!($varname)).unwrap_or(&null);
                            jsonrpc_params!(subval, single $varname: $vartype)?
                        },
                    )+
                )
            },
            Some(_) => {
                return Err($crate::message::RpcError::
                           invalid_params(Some("Expected an object as parameters".to_owned())));
            },
        }
    }};
    // Decode params, decide if named or positional based on what arrived
    ( $value:expr, decide $( $varname:ident : $vartype:ty ),+ ) => {{
        let val: &Option<$crate::macro_exports::Value> = $value;
        match *val {
            None => return Err($crate::message::RpcError::
                               invalid_params(Some("Expected parameters".to_owned()))),
            Some(Value::Array(_)) => jsonrpc_params!(val, positional $( $varname: $vartype ),+),
            Some(Value::Object(_)) => jsonrpc_params!(val, named $( $varname: $vartype ),+),
            Some(_) => {
                return Err($crate::message::RpcError::
                           invalid_params(Some("Expected an object or an array as parameters"
                                               .to_owned())))
            },
        }
    }};
    // A special case for a single param.
    //
    // We allow decoding it directly, mostly to support users with a complex all-params structure.
    ( $value:expr, $varname: ident: $vartype: ty ) => {{
        let val: &Option<$crate::macro_exports::Value> = $value;
        // First try decoding directly
        let single = val.as_ref().map(|val| jsonrpc_params!(val, single $varname: $vartype));
        if let Some(Ok(result)) = single {
            (result,)
        } else {
            // If direct single decoding didn't work, try the usual multi-param way.
            jsonrpc_params!(val, decide $varname: $vartype)
        }
    }};
    // Propagate multiple params.
    ( $value:expr, $( $varname:ident : $vartype:ty ),+ ) => {
        jsonrpc_params!($value, decide $( $varname: $vartype ),+)
    };
}

/*
 The intention:

 jsonrpc_server! {
    X {
        rpcs {
            hello(i: usize); // Will call x.hello(i), convert parameters, convert result…
        }
        notifications {
            hi(x: String); // Will call x.hi(…)
        }
        init // Will call x.init
    }
 }


   */

/*
trace_macros!(true);
// TODO: We want to be able to accept arrays of different kinds of data, possibly alternatives…
macro_rules! json_param {
    ( (), $value:ident ) => { () };
    ( $param:ty, $value:ident ) => {
        match *$value {
            None => unimplemented!(),
            Some(ref v) => {
                let result: Result<$param, _> = from_value(v.clone());
                match result {
                    Ok(r) => r,
                    Err(_) => unimplemented!(),
                }
            },
        }
    }
}
macro_rules! json_rpc_impl {
    ( $( $method:pat => ($param:ty) $code:block ),* ) => {
        // TODO Use $crate for the types and absolute paths for Value
        fn rpc(&self, ctl: &ServerCtl, method: &str, param: &Option<Value>) ->
        Option<Self::RpcCallResult> {
            match method {
                $( $method => {
                    let input = json_param!($param, param);
                    let result = $code;
                    let mapped = result.map(|r| to_value(r).expect("Error converting RPC result"));
                    Some(Box::new(mapped.into_future()))
                }, )*
                _ => None,
            }
        }
    };
}

    struct X;

    impl Server for X {
        type Success = Value;
        type RpcCallResult = BoxRpcCallResult;
        type NotificationResult = BoxNotificationResult;
        json_rpc_impl!{
            "test" => (usize) {
                Ok(42)
            },
            "another" => (bool) {
                Ok("Hello".to_owned())
            }
        }
    }
    */

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use serde_json::Map;

    use super::*;

    /// Check the empty server is somewhat sane.
    #[test]
    fn empty() {
        let server = Empty;
        let (ctl, dropped, _killed) = ServerCtl::new_test();
        // As we can't reasonably check all possible method names, do so for just a bunch
        for method in ["method", "notification", "check"].iter() {
            assert!(server.rpc(&ctl, method, &None).is_none());
            assert!(server.notification(&ctl, method, &None).is_none());
        }
        // It terminates the ctl on the server side on initialization
        server.initialized(&ctl);
        dropped.wait().unwrap();
    }

    /// A server that logs what has been called.
    #[derive(Default, Debug, PartialEq)]
    struct LogServer {
        serial: Cell<usize>,
        rpc: RefCell<Vec<usize>>,
        notification: RefCell<Vec<usize>>,
        initialized: RefCell<Vec<usize>>,
    }

    impl LogServer {
        fn update(&self, what: &RefCell<Vec<usize>>) {
            let serial = self.serial.get() + 1;
            self.serial.set(serial);
            what.borrow_mut().push(serial);
        }
    }

    impl Server for LogServer {
        type Success = bool;
        type RpcCallResult = Result<bool, RpcError>;
        type NotificationResult = Result<(), ()>;
        fn rpc(&self, _ctl: &ServerCtl, method: &str, params: &Option<Value>)
               -> Option<Self::RpcCallResult> {
            self.update(&self.rpc);
            match method {
                "test" => {
                    assert!(params.is_none());
                    Some(Ok(true))
                },
                _ => None,
            }
        }
        fn notification(&self, _ctl: &ServerCtl, method: &str, params: &Option<Value>)
                        -> Option<Self::NotificationResult> {
            self.update(&self.notification);
            assert!(params.is_none());
            match method {
                "notification" => Some(Ok(())),
                _ => None,
            }
        }
        fn initialized(&self, _ctl: &ServerCtl) {
            self.update(&self.initialized);
        }
    }

    /// Testing of the abstract server
    ///
    /// Just checking the data gets through and calling everything, there's nothing much to test
    /// anyway.
    #[test]
    fn abstract_server() {
        let log_server = LogServer::default();
        let abstract_server = AbstractServer::new(log_server);
        let (ctl, _, _) = ServerCtl::new_test();
        let rpc_result = abstract_server.rpc(&ctl, "test", &None)
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(Value::Bool(true), rpc_result);
        abstract_server.notification(&ctl, "notification", &None)
            .unwrap()
            .wait()
            .unwrap();
        assert!(abstract_server.rpc(&ctl, "another", &None).is_none());
        assert!(abstract_server.notification(&ctl, "another", &None).is_none());
        abstract_server.initialized(&ctl);
        let log_server = abstract_server.into_inner();
        let expected = LogServer {
            serial: Cell::new(5),
            rpc: RefCell::new(vec![1, 3]),
            notification: RefCell::new(vec![2, 4]),
            initialized: RefCell::new(vec![5]),
        };
        assert_eq!(expected, log_server);
    }

    struct AnotherServer;

    impl Server for AnotherServer {
        type Success = usize;
        type RpcCallResult = Result<usize, RpcError>;
        type NotificationResult = Result<(), ()>;
        fn rpc(&self, _ctl: &ServerCtl, method: &str, params: &Option<Value>)
               -> Option<Self::RpcCallResult> {
            assert!(params.as_ref()
                        .unwrap()
                        .is_null());
            match method {
                "another" => Some(Ok(42)),
                _ => None,
            }
        }
        // Ignore the other methods
    }

    /// Test the chain.
    ///
    /// By the asserts on params in the servers we check that only whan should be called is.
    #[test]
    fn chain() {
        let empty_server = Empty;
        let log_server = LogServer::default();
        let another_server = AnotherServer;
        let (ctl, dropped, _killed) = ServerCtl::new_test();
        let chain = ServerChain::new(vec![Box::new(AbstractServer::new(empty_server)),
                                          Box::new(AbstractServer::new(log_server)),
                                          Box::new(AbstractServer::new(another_server))]);
        chain.initialized(&ctl);
        dropped.wait().unwrap();
        assert_eq!(Value::Bool(true),
                   chain.rpc(&ctl, "test", &None)
                       .unwrap()
                       .wait()
                       .unwrap());
        assert_eq!(json!(42),
                   chain.rpc(&ctl, "another", &Some(Value::Null))
                       .unwrap()
                       .wait()
                       .unwrap());
        assert!(chain.rpc(&ctl, "wrong", &Some(Value::Null)).is_none());
        chain.notification(&ctl, "notification", &None)
            .unwrap()
            .wait()
            .unwrap();
        assert!(chain.notification(&ctl, "another", &None).is_none());
        // It would be great to check what is logged inside the log server. But downcasting a trait
        // object seems to be a big pain and probably isn't worth it here.
    }

    /// A guard object that panics when dropped unless it has been disarmed first.
    ///
    /// We use it to check the macro we test didn't short-circuit the test by returning early. Note
    /// that it causes a double panic if the test fails (in that case you want to temporarily
    /// remove the panic guard from that test).
    ///
    /// Most of the following tests don't need it, as they call the macro indirectly, by wrapping
    /// it into a function (and such function can't return in the caller).
    struct PanicGuard(bool);

    impl PanicGuard {
        /// A constructor. Creates an armed guerd.
        fn new() -> Self {
            PanicGuard(true)
        }
        /// Disarm the guard → it won't panic when dropped.
        fn disarm(&mut self) {
            self.0 = false;
        }
    }

    impl Drop for PanicGuard {
        fn drop(&mut self) {
            if self.0 {
                panic!("PanicGuard dropped without being disarmed");
            }
        }
    }

    /// Test the panic guard itself
    #[test]
    #[should_panic]
    fn panic_guard() {
        PanicGuard::new();
    }

    /// Expect no params and return whanever we got from the macro.
    ///
    /// It is a separate function so the return error thing from the macro doesn't end the test
    /// prematurely (actually, it wouldn't, as the return type doesn't match).
    fn expect_no_params(params: &Option<Value>) -> Result<(), RpcError> {
        // Check that we can actually assign it somewhere (this may be needed in other macros later
        // on.
        let () = jsonrpc_params!(params, );
        Ok(())
    }

    /// Test the jsonrpc_params macro when we expect no parameters.
    #[test]
    fn params_macro_none() {
        // These are legal no-params, at least for us
        expect_no_params(&None).unwrap();
        expect_no_params(&Some(Value::Null)).unwrap();
        expect_no_params(&Some(Value::Array(Vec::new()))).unwrap();
        expect_no_params(&Some(Value::Object(Map::new()))).unwrap();
        // Some illegal values
        expect_no_params(&Some(Value::Bool(true))).unwrap_err();
        expect_no_params(&Some(json!([42, "hello"]))).unwrap_err();
        expect_no_params(&Some(json!({"hello": 42}))).unwrap_err();
        expect_no_params(&Some(json!(42))).unwrap_err();
        expect_no_params(&Some(json!("hello"))).unwrap_err();
    }

    /// Test the single-param jsonrpc_params helper variant.
    #[test]
    fn single_param() {
        let mut guard = PanicGuard::new();
        // A valid conversion
        // Make sure the return type fits
        let result: Result<bool, RpcError> =
            jsonrpc_params!(&Value::Bool(true), single param: bool);
        assert!(result.unwrap());
        // Some invalid conversions
        jsonrpc_params!(&Value::Null, single param: bool).unwrap_err();
        jsonrpc_params!(&Value::Array(Vec::new()), single param: bool).unwrap_err();
        guard.disarm();
    }

    /// A helper function to decode two values as positional arguments.
    ///
    /// This is to prevent attempt to return errors from within the test function.
    fn bool_str_positional(value: &Option<Value>) -> Result<(bool, String), RpcError> {
        let (b, s) = jsonrpc_params!(value, positional b: bool, s: String);
        Ok((b, s))
    }

    /// Like above, but with only a single variable.
    ///
    /// As single-values are handled slightly differently at a syntax level (eg, a tuple with only
    /// one element needs a terminating comma) and also differently in the macro (they are
    /// sometimes the ends of recursion), we mostly want to check it compiles.
    ///
    /// It also checks we don't get confused with an array inside the parameter array.
    fn single_positional(value: &Option<Value>) -> Result<Vec<String>, RpcError> {
        let (r,) = jsonrpc_params!(value, positional arr: Vec<String>);
        Ok(r)
    }

    /// Test decoding positional arguments.
    #[test]
    fn positional() {
        // Some that don't match
        bool_str_positional(&None).unwrap_err();
        bool_str_positional(&Some(Value::Null)).unwrap_err();
        bool_str_positional(&Some(Value::Bool(true))).unwrap_err();
        bool_str_positional(&Some(json!({"b": true, "s": "hello"}))).unwrap_err();
        bool_str_positional(&Some(json!([true]))).unwrap_err();
        bool_str_positional(&Some(json!([true, "hello", false]))).unwrap_err();
        bool_str_positional(&Some(json!([true, true]))).unwrap_err();
        // This one should be fine
        assert_eq!((true, "hello".to_owned()),
                   bool_str_positional(&Some(json!([true, "hello"]))).unwrap());

        single_positional(&None).unwrap_err();
        // We need two nested arrays
        single_positional(&Some(json!(["Hello"]))).unwrap_err();
        assert!(single_positional(&Some(json!([[]]))).unwrap().is_empty());
        assert_eq!(vec!["hello", "world"],
                   single_positional(&Some(json!([["hello", "world"]]))).unwrap());
    }

    /// Helper function to decode two values as named arguments
    fn bool_str_named(value: &Option<Value>) -> Result<(bool, String), RpcError> {
        let (b, s) = jsonrpc_params!(value, named b: bool, s: String);
        Ok((b, s))
    }

    #[derive(Deserialize, Debug, Eq, PartialEq)]
    struct TestStruct {
        x: i32,
    }

    /// Like above, but with only one parameter.
    fn single_named(value: &Option<Value>) -> Result<TestStruct, RpcError> {
        let (ts,) = jsonrpc_params!(value, named ts: TestStruct);
        Ok(ts)
    }

    /// Test an optional value might be missing.
    fn optional_named(value: &Option<Value>) -> Result<Option<u32>, RpcError> {
        let (ov,) = jsonrpc_params!(value, named ov: Option<u32>);
        Ok(ov)
    }

    /// Test decoding named arguments
    #[test]
    fn named() {
        bool_str_named(&None).unwrap_err();
        bool_str_named(&Some(Value::Null)).unwrap_err();
        bool_str_named(&Some(Value::Bool(true))).unwrap_err();
        bool_str_named(&Some(json!([true, "hello"]))).unwrap_err();
        bool_str_named(&Some(json!({"b": true, "s": 42}))).unwrap_err();
        // FIXME: This fails, as serde_json considers Value::Null to be an empty string
        //bool_str_named(&Some(json!({"b": true}))).unwrap_err();
        bool_str_named(&Some(json!({"s": "hello"}))).unwrap_err();
        assert_eq!((true, "hello".to_owned()),
                   bool_str_named(&Some(json!({"b": true, "s": "hello"}))).unwrap());
        // FIXME: We currently don't know how to check against extra params
        assert_eq!((true, "hello".to_owned()),
                   bool_str_named(&Some(json!({"b": true, "s": "hello", "x": 42}))).unwrap());

        single_named(&None).unwrap_err();
        single_named(&Some(json!({"ts": 42}))).unwrap_err();
        single_named(&Some(json!({"ts": {"x": 42}}))).unwrap();

        optional_named(&None).unwrap_err();
        optional_named(&Some(json!([]))).unwrap_err();
        assert_eq!(Some(42), optional_named(&Some(json!({"ov": 42}))).unwrap());
        assert_eq!(None, optional_named(&Some(json!({}))).unwrap());
    }

    /// A helper function to decode two parameters.
    ///
    /// The decoding decides how to do so based on what arrived.
    fn bool_str(value: &Option<Value>) -> Result<(bool, String), RpcError> {
        let (b, s) = jsonrpc_params!(value, b: bool, s: String);
        Ok((b, s))
    }

    /// Test decoding parameters when it decides itself how.
    #[test]
    fn decide() {
        bool_str(&None).unwrap_err();
        bool_str(&Some(Value::Null)).unwrap_err();
        bool_str(&Some(Value::Bool(true))).unwrap_err();
        assert_eq!((true, "hello".to_owned()),
                   bool_str_named(&Some(json!({"b": true, "s": "hello"}))).unwrap());
        assert_eq!((true, "hello".to_owned()),
                   bool_str_positional(&Some(json!([true, "hello"]))).unwrap());
    }

    /// A helper for the `decide_single` test.
    fn decode_test_struct(value: &Option<Value>) -> Result<TestStruct, RpcError> {
        let (ts,) = jsonrpc_params!(value, ts: TestStruct);
        Ok(ts)
    }

    /// Similar to `decide`, but with a single parameter.
    ///
    /// The single parameter is special, since it can decode the parameters structure directly.
    /// This is to support the user having an all-encompassing parameter struct (possibly with all
    /// optional/default/renaming tweaks done through fine-tuning serde).
    #[test]
    fn decide_single() {
        decode_test_struct(&None).unwrap_err();
        decode_test_struct(&Some(Value::Null)).unwrap_err();
        decode_test_struct(&Some(Value::Bool(true))).unwrap_err();

        // Encoded as an array
        assert_eq!(TestStruct { x: 42 }, decode_test_struct(&Some(json!([{"x": 42}]))).unwrap());
        // Encoded as an object
        assert_eq!(TestStruct { x: 42 },
                   decode_test_struct(&Some(json!({"ts": {"x": 42}}))).unwrap());
        // Encoded directly as the parameters structure
        assert_eq!(TestStruct { x: 42 }, decode_test_struct(&Some(json!({"x": 42}))).unwrap());
    }
}
