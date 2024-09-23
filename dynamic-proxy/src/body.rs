use bytes::Bytes;
use http_body::Body;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};

pub type BoxedError = Box<dyn std::error::Error + Send + Sync>;

pub type SimpleBody = BoxBody<Bytes, BoxedError>;

pub fn to_simple_body<B>(body: B) -> SimpleBody
where
    B: Body<Data = Bytes> + Send + Sync + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    body.map_err(|e| e.into() as Box<dyn std::error::Error + Send + Sync + 'static>)
        .boxed()
}

pub fn simple_empty_body() -> SimpleBody {
    Empty::<Bytes>::new()
        .map_err(|_| unreachable!("Infallable"))
        .boxed()
}
