use std::io::{self, ErrorKind};

use rand;

use futures::Future;
use futures::sync::{oneshot, mpsc};

use hyper::{self, StatusCode};
use hyper::header::ContentLength;
use hyper::server::{Response, Request};

use rmpv::ValueRef;

use cocaine::{Dispatch, Error, Service};
use cocaine::logging::Logger;

use logging::AccessLogger;
use pool::Event;
use route::Route;

pub struct PerformanceRoute {
    txs: Vec<mpsc::UnboundedSender<Event>>,
    log: Logger,
}

impl PerformanceRoute {
    pub fn new(txs: Vec<mpsc::UnboundedSender<Event>>, log: Logger) -> Self {
        Self {
            txs: txs,
            log: log,
        }
    }
}

impl Route for PerformanceRoute {
    type Future = Box<Future<Item = Response, Error = hyper::Error>>;

    fn process(&self, req: &Request) -> Option<Self::Future> {
        let (tx, rx) = oneshot::channel();

        let ev = Event::Service {
            name: "geobase".into(),
            func: box move |service: &Service| {
                let future = service.call(0, &vec!["8.8.8.8"], SingleChunkReadDispatch { tx: tx })
                    .then(|tx| {
                        drop(tx);
                        Ok(())
                    });
                future.boxed()
            },
        };

        let x = rand::random::<usize>();
        let rolled = x % self.txs.len();
        self.txs[rolled].send(ev).unwrap();

        let log = AccessLogger::new(self.log.clone(), req);
        let future = rx.and_then(move |(mut res, bytes_sent)| {
            res.headers_mut().set_raw("X-Powered-By", "Cocaine");
            log.commit(x, res.status().into(), bytes_sent);
            Ok(res)
        }).map_err(|err| hyper::Error::Io(io::Error::new(ErrorKind::Other, format!("{}", err))));

        Some(future.boxed())
    }
}

struct SingleChunkReadDispatch {
    tx: oneshot::Sender<(Response, u64)>,
}

impl Dispatch for SingleChunkReadDispatch {
    fn process(self: Box<Self>, ty: u64, data: &ValueRef) -> Option<Box<Dispatch>> {
        let (code, body) = match ty {
            0 => {
                (200, format!("{}", data))
            }
            1 => {
                (500, format!("{}", data))
            }
            m => {
                (500, format!("unknown type: {} {}", m, data))
            }
        };

        let body_len = body.as_bytes().len() as u64;

        let mut res = Response::new();
        res.set_status(StatusCode::from_u16(code as u16));
        res.headers_mut().set(ContentLength(body_len));
        res.set_body(body);

        drop(self.tx.send((res, body_len)));

        None
    }

    fn discard(self: Box<Self>, err: &Error) {
        let body = format!("{}", err);
        let body_len = body.as_bytes().len() as u64;

        let mut res = Response::new();
        res.set_status(StatusCode::InternalServerError);
        res.set_body(body);

        drop(self.tx.send((res, body_len)));
    }
}