use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, time::SystemTime};
use time::OffsetDateTime;
use valuable::{Tuplable, TupleDef, Valuable, Value, Visit};

// See: https://github.com/tokio-rs/valuable/issues/86#issuecomment-1760446976

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LoggableTime(#[serde(with = "chrono::serde::ts_milliseconds")] pub DateTime<Utc>);

impl Valuable for LoggableTime {
    fn as_value(&self) -> Value<'_> {
        Value::Tuplable(self)
    }

    fn visit(&self, visit: &mut dyn Visit) {
        let s: String = format!("{}", self.0);
        let val = Value::String(s.as_str());
        visit.visit_unnamed_fields(&[val]);
    }
}

impl Tuplable for LoggableTime {
    fn definition(&self) -> TupleDef {
        TupleDef::new_static(1)
    }
}

impl From<OffsetDateTime> for LoggableTime {
    fn from(offset: OffsetDateTime) -> Self {
        let t: SystemTime = offset.into();
        let dt: DateTime<Utc> = t.into();
        Self(dt)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd)]
pub struct BackendAddr(pub SocketAddr);

impl valuable::Valuable for BackendAddr {
    fn as_value(&self) -> valuable::Value {
        Value::Tuplable(self)
    }

    fn visit(&self, visit: &mut dyn valuable::Visit) {
        let s: String = format!("{:?}", self.0);
        let val = valuable::Value::String(s.as_str());
        visit.visit_unnamed_fields(&[val]);
    }
}

impl Tuplable for BackendAddr {
    fn definition(&self) -> valuable::TupleDef {
        valuable::TupleDef::new_static(1)
    }
}
