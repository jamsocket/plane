use crate::{
    log_types::LoggableTime,
    names::BackendName,
    protocol::{BackendEventId, BackendMetricsMessage, BackendStateMessage},
    typed_socket::TypedSocketSender,
    types::BackendState,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::{Arc, RwLock};

/// An array of sqlite commands used to initialize the state store.
/// These must be idempotent, because they are run every time a state store
/// is initialized.
const SCHEMA: &[&str] = &[
    r#"
        create table if not exists "backend" (
            "id" text primary key,
            "state" json not null
        );
    "#,
    r#"
        create table if not exists "event" (
            "id" integer primary key autoincrement,
            "backend_id" text,
            "event" json not null,
            "timestamp" integer not null,
            foreign key ("backend_id") references "backend"("id")
        );
    "#,
];

/// Stores state information about running backends.
pub struct StateStore {
    db_conn: Connection,

    /// A function that is called when a backend's state changes.
    listener: Option<Box<dyn Fn(BackendStateMessage) + Send + Sync + 'static>>,

    /// Sender for metrics
    /// NOTE: downstream code assumes that this sender is refreshed on reconnect
    metrics: Option<Arc<RwLock<TypedSocketSender<BackendMetricsMessage>>>>,
}

impl StateStore {
    pub fn new(db_conn: Connection) -> Result<Self> {
        for table in SCHEMA {
            db_conn.execute(table, [])?;
        }

        Ok(Self {
            db_conn,
            listener: None,
            metrics: None,
        })
    }

    /// Make the state store aware of a change to a backend's state.
    pub fn register_event(
        &mut self,
        backend_id: &BackendName,
        state: &BackendState,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let tx = self.db_conn.transaction()?;

        // "Upsert" the current backend state into the table. Per sqlite docs (https://www.sqlite.org/lang_upsert.html):
        // > Column names in the expressions of a DO UPDATE refer to the original unchanged value of the column,
        // > before the attempted INSERT. To use the value that would have been inserted had the constraint not
        // > failed, add the special "excluded." table qualifier to the column name.

        tx.execute(
            r#"
                insert into "backend" (
                    "id",
                    "state"
                )
                values (?, ?)
                on conflict ("id")
                do update set
                    "state" = excluded."state"
            "#,
            (backend_id.to_string(), serde_json::to_value(state)?),
        )?;

        tx.execute(
            r#"
                insert into "event" (
                    "backend_id",
                    "event",
                    "timestamp"
                ) values (?, ?, ?)
            "#,
            (
                backend_id.to_string(),
                serde_json::to_value(state)?,
                timestamp.timestamp_millis(),
            ),
        )?;

        tx.commit()?;

        if let Some(listener) = &self.listener {
            let event_id = BackendEventId::from(self.db_conn.last_insert_rowid());
            let event_message = BackendStateMessage {
                event_id,
                backend_id: backend_id.clone(),
                timestamp: LoggableTime(timestamp),
                state: state.clone(),
            };

            listener(event_message);
        }

        Ok(())
    }

    pub fn backend_state(&self, backend_id: &BackendName) -> Result<BackendState> {
        let mut stmt = self.db_conn.prepare(
            r#"
                select "state"
                from "backend"
                where id = ?
                limit 1
            "#,
        )?;

        let mut rows = stmt.query([backend_id.to_string()])?;

        let row = rows.next()?.ok_or_else(|| {
            anyhow::anyhow!(
                "No backend with id {} found in state store.",
                backend_id.to_string()
            )
        })?;

        let state: String = row.get(0)?;
        let state: BackendState = serde_json::from_str(&state)?;

        Ok(state)
    }

    fn unacked_events(&self) -> Result<Vec<BackendStateMessage>> {
        let mut stmt = self.db_conn.prepare(
            r#"
                select
                    id,
                    backend_id,
                    event,
                    timestamp
                from "event"
                order by timestamp asc
            "#,
        )?;

        let mut rows = stmt.query([])?;
        let mut result = Vec::new();

        while let Some(row) = rows.next()? {
            let event_id: i64 = row.get(0)?;
            let backend_id: String = row.get(1)?;
            let state: String = row.get(2)?;
            let timestamp: i64 = row.get(3)?;

            let state: BackendState = serde_json::from_str(&state)?;

            let event = BackendStateMessage {
                event_id: BackendEventId::from(event_id),
                backend_id: BackendName::try_from(backend_id)?,
                state: state.clone(),
                timestamp: LoggableTime(
                    DateTime::UNIX_EPOCH + chrono::Duration::milliseconds(timestamp),
                ),
            };

            result.push(event);
        }

        Ok(result)
    }

    pub fn register_listener<F>(&mut self, listener: F) -> Result<()>
    where
        F: Fn(BackendStateMessage) + Send + Sync + 'static,
    {
        // We assume that events that have been sent but not acked are now dropped,
        // so we replay them here.
        for event in self.unacked_events()? {
            listener(event);
        }

        self.listener = Some(Box::new(listener));

        Ok(())
    }

    pub fn register_metrics_sender(&mut self, sender: TypedSocketSender<BackendMetricsMessage>) {
        if let Some(ref metrics) = self.metrics {
            *metrics
                .write()
                .expect("backend metrics sender lock poisoned!") = sender;
        } else {
            self.metrics = Some(Arc::new(RwLock::new(sender)));
        }
    }

    pub fn get_metrics_sender(
        &self,
    ) -> Result<Arc<RwLock<TypedSocketSender<BackendMetricsMessage>>>> {
        self.metrics
            .clone()
            .ok_or_else(|| anyhow::anyhow!("metrics sender not initialized"))
    }

    pub fn ack_event(&self, event_id: BackendEventId) -> Result<()> {
        self.db_conn.execute(
            r#"
                delete from "event"
                where id = ?
            "#,
            (i64::from(event_id),),
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        log_types::BackendAddr,
        names::Name,
        types::{BackendStatus, TerminationKind, TerminationReason},
    };
    use std::{
        net::{SocketAddr, SocketAddrV4},
        sync::mpsc,
    };

    fn dummy_addr() -> BackendAddr {
        BackendAddr(SocketAddr::V4(SocketAddrV4::new(
            "12.34.12.34".parse().unwrap(),
            1234,
        )))
    }

    #[test]
    fn single_event() {
        let conn = Connection::open_in_memory().unwrap();
        let mut state_store = StateStore::new(conn).unwrap();
        let backend_id = BackendName::new_random();

        state_store
            .register_event(
                &backend_id,
                &BackendState::Ready {
                    address: Some(dummy_addr()),
                },
                Utc::now(),
            )
            .unwrap();

        let result = state_store.backend_state(&backend_id).unwrap();
        assert_eq!(
            result,
            BackendState::Ready {
                address: Some(dummy_addr())
            }
        );
    }

    #[test]
    fn two_events() {
        let conn = Connection::open_in_memory().unwrap();
        let mut state_store = StateStore::new(conn).unwrap();
        let backend_id = BackendName::new_random();

        let ready_state = BackendState::Ready {
            address: Some(dummy_addr()),
        };
        {
            state_store
                .register_event(&backend_id, &ready_state, Utc::now())
                .unwrap();

            let result = state_store.backend_state(&backend_id).unwrap();
            assert_eq!(
                result,
                BackendState::Ready {
                    address: Some(dummy_addr())
                }
            );
        }

        {
            state_store
                .register_event(
                    &backend_id,
                    &ready_state.to_terminating(TerminationKind::Hard, TerminationReason::External),
                    Utc::now(),
                )
                .unwrap();

            let result = state_store.backend_state(&backend_id).unwrap();
            assert_eq!(
                result,
                BackendState::Terminating {
                    last_status: BackendStatus::Ready,
                    termination: TerminationKind::Hard,
                    reason: TerminationReason::External,
                }
            );
        }
    }

    #[test]
    fn subscribe_events() {
        let (send, recv) = mpsc::channel::<BackendStateMessage>();

        let conn = Connection::open_in_memory().unwrap();
        let mut state_store = StateStore::new(conn).unwrap();

        state_store
            .register_listener(move |event| {
                send.send(event).unwrap();
            })
            .unwrap();

        let backend_id = BackendName::new_random();

        let ready_state = BackendState::Ready {
            address: Some(dummy_addr()),
        };
        state_store
            .register_event(&backend_id, &ready_state, Utc::now())
            .unwrap();

        {
            let result = state_store.backend_state(&backend_id).unwrap();
            assert_eq!(result, ready_state);

            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(
                event.state,
                BackendState::Ready {
                    address: Some(dummy_addr())
                }
            );
        }

        {
            state_store
                .register_event(
                    &backend_id,
                    &ready_state.to_terminating(TerminationKind::Hard, TerminationReason::Swept),
                    Utc::now(),
                )
                .unwrap();

            let result = state_store.backend_state(&backend_id).unwrap();
            assert_eq!(
                result,
                ready_state.to_terminating(TerminationKind::Hard, TerminationReason::Swept)
            );

            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(
                event.state,
                BackendState::Terminating {
                    last_status: BackendStatus::Ready,
                    termination: TerminationKind::Hard,
                    reason: TerminationReason::Swept,
                }
            );
        }
    }

    #[test]
    fn events_are_durable() {
        let (send, recv) = mpsc::channel::<BackendStateMessage>();

        let conn = Connection::open_in_memory().unwrap();
        let mut state_store = StateStore::new(conn).unwrap();

        let backend_id = BackendName::new_random();

        let ready_state = BackendState::Ready {
            address: Some(dummy_addr()),
        };
        state_store
            .register_event(&backend_id, &ready_state, Utc::now())
            .unwrap();

        state_store
            .register_event(
                &backend_id,
                &ready_state.to_terminating(TerminationKind::Hard, TerminationReason::Swept),
                Utc::now(),
            )
            .unwrap();

        state_store
            .register_listener(move |event| {
                send.send(event).unwrap();
            })
            .unwrap();

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.event_id, BackendEventId::from(1));
            assert_eq!(
                event.state,
                BackendState::Ready {
                    address: Some(dummy_addr())
                }
            );
        }

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.event_id, BackendEventId::from(2));
            assert_eq!(
                event.state,
                BackendState::Terminating {
                    last_status: BackendStatus::Ready,
                    termination: TerminationKind::Hard,
                    reason: TerminationReason::Swept,
                }
            );
        }

        assert!(recv.try_recv().is_err());

        // Events are replayed when we install a new listener.
        let (send, recv) = mpsc::channel::<BackendStateMessage>();
        state_store
            .register_listener(move |event| {
                send.send(event).unwrap();
            })
            .unwrap();

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(
                event.state,
                BackendState::Ready {
                    address: Some(dummy_addr())
                }
            );
        }

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(
                event.state,
                BackendState::Terminating {
                    last_status: BackendStatus::Ready,
                    termination: TerminationKind::Hard,
                    reason: TerminationReason::Swept,
                }
            );
        }

        assert!(recv.try_recv().is_err());

        // Events are NOT replayed once acked.
        let (send, recv) = mpsc::channel::<BackendStateMessage>();

        state_store.ack_event(BackendEventId::from(1)).unwrap();

        state_store
            .register_listener(move |event| {
                send.send(event).unwrap();
            })
            .unwrap();

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(
                event.state,
                BackendState::Terminating {
                    last_status: BackendStatus::Ready,
                    termination: TerminationKind::Hard,
                    reason: TerminationReason::Swept,
                }
            );
        }

        assert!(recv.try_recv().is_err());
    }
}
