use crate::{
    names::BackendName,
    protocol::{BackendEventId, BackendStateMessage},
    types::BackendState,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;

const SCHEMA: &[&str] = &[
    r#"
        create table if not exists "backend" (
            "id" text primary key,
            "status" json not null
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
    listener: Option<Box<dyn Fn(BackendStateMessage) + Send + Sync + 'static>>,
}

impl StateStore {
    pub fn new(db_conn: Connection) -> Result<Self> {
        for table in SCHEMA {
            db_conn.execute(table, [])?;
        }

        Ok(Self {
            db_conn,
            listener: None,
        })
    }

    pub fn register_event(
        &self,
        backend_id: &BackendName,
        state: &BackendState,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.db_conn.execute(
            r#"
                insert into "backend" (
                    "id",
                    "status"
                )
                values (?, ?)
                on conflict ("id")
                do update set
                    "status" = excluded."status"
            "#,
            (backend_id.to_string(), serde_json::to_value(state)?),
        )?;

        self.db_conn.execute(
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

        if let Some(listener) = &self.listener {
            let event_id = BackendEventId::from(self.db_conn.last_insert_rowid());
            let event_message = BackendStateMessage {
                event_id,
                backend_id: backend_id.clone(),
                status: state.status,
                address: state.address,
                exit_code: state.exit_code,
                timestamp,
            };

            listener(event_message);
        }

        Ok(())
    }

    pub fn backend_status(&self, backend_id: &BackendName) -> Result<BackendState> {
        let mut stmt = self.db_conn.prepare(
            r#"
                select "status"
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

        let status: String = row.get(0)?;
        let status: BackendState = serde_json::from_str(&status)?;

        Ok(status)
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
                status: state.status,
                address: state.address,
                exit_code: state.exit_code,
                timestamp: DateTime::UNIX_EPOCH + chrono::Duration::milliseconds(timestamp),
            };

            result.push(event);
        }

        Ok(result)
    }

    pub fn register_listener<F>(&mut self, listener: F) -> Result<()>
    where
        F: Fn(BackendStateMessage) + Send + Sync + 'static,
    {
        for event in self.unacked_events()? {
            listener(event);
        }

        self.listener = Some(Box::new(listener));

        Ok(())
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
        names::Name,
        types::{BackendState, BackendStatus, TerminationKind},
    };
    use std::sync::mpsc;

    fn simple_backend_state(status: BackendStatus) -> BackendState {
        BackendState {
            status,
            address: None,
            exit_code: None,
            termination: None,
        }
    }

    #[test]
    fn single_event() {
        let conn = Connection::open_in_memory().unwrap();
        let state_store = StateStore::new(conn).unwrap();
        let backend_id = BackendName::new_random();

        state_store
            .register_event(
                &backend_id,
                &simple_backend_state(BackendStatus::Ready),
                Utc::now(),
            )
            .unwrap();

        let result = state_store.backend_status(&backend_id).unwrap();
        assert_eq!(result, simple_backend_state(BackendStatus::Ready));
    }

    #[test]
    fn two_events() {
        let conn = Connection::open_in_memory().unwrap();
        let state_store = StateStore::new(conn).unwrap();
        let backend_id = BackendName::new_random();

        {
            state_store
                .register_event(
                    &backend_id,
                    &simple_backend_state(BackendStatus::Ready),
                    Utc::now(),
                )
                .unwrap();

            let result = state_store.backend_status(&backend_id).unwrap();
            assert_eq!(result, simple_backend_state(BackendStatus::Ready));
        }

        {
            state_store
                .register_event(
                    &backend_id,
                    &BackendState::terminating(TerminationKind::Hard),
                    Utc::now(),
                )
                .unwrap();

            let result = state_store.backend_status(&backend_id).unwrap();
            assert_eq!(result, BackendState::terminating(TerminationKind::Hard));
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

        state_store
            .register_event(
                &backend_id,
                &simple_backend_state(BackendStatus::Ready),
                Utc::now(),
            )
            .unwrap();

        {
            let result = state_store.backend_status(&backend_id).unwrap();
            assert_eq!(result, simple_backend_state(BackendStatus::Ready));

            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.status, BackendStatus::Ready);
        }

        {
            state_store
                .register_event(
                    &backend_id,
                    &BackendState::terminating(TerminationKind::Hard),
                    Utc::now(),
                )
                .unwrap();

            let result = state_store.backend_status(&backend_id).unwrap();
            assert_eq!(result, BackendState::terminating(TerminationKind::Hard));

            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.status, BackendStatus::Terminating,);
        }
    }

    #[test]
    fn events_are_durable() {
        let (send, recv) = mpsc::channel::<BackendStateMessage>();

        let conn = Connection::open_in_memory().unwrap();
        let mut state_store = StateStore::new(conn).unwrap();

        let backend_id = BackendName::new_random();

        state_store
            .register_event(
                &backend_id,
                &simple_backend_state(BackendStatus::Ready),
                Utc::now(),
            )
            .unwrap();

        state_store
            .register_event(
                &backend_id,
                &BackendState::terminating(TerminationKind::Hard),
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
            assert_eq!(event.status, BackendStatus::Ready);
        }

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.event_id, BackendEventId::from(2));
            assert_eq!(event.status, BackendStatus::Terminating,);
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
            assert_eq!(event.status, BackendStatus::Ready);
        }

        {
            let event = recv.try_recv().unwrap();
            assert_eq!(event.backend_id, backend_id);
            assert_eq!(event.status, BackendStatus::Terminating,);
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
            assert_eq!(event.status, BackendStatus::Terminating,);
        }

        assert!(recv.try_recv().is_err());
    }
}
