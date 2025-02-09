//! Provides a concrete, boxed body and error type.

use bytes::Bytes;
use http_body::Body;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};

pub type BoxedError = Box<dyn std::error::Error + Send + Sync>;

pub type SimpleBody = UnsyncBoxBody<Bytes, BoxedError>;

pub fn to_simple_body<B>(body: B) -> SimpleBody
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxedError>,
{
    body.map_err(|e| e.into() as BoxedError).boxed_unsync()
}

pub fn simple_empty_body() -> SimpleBody {
    Empty::<Bytes>::new()
        .map_err(|_| unreachable!("Infallable"))
        .boxed_unsync()
}
