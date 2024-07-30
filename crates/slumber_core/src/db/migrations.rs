use crate::{
    db::{
        convert::{ByteEncoded, SqlWrap},
        CollectionId,
    },
    http::Exchange,
    util::ResultTraced,
};
use anyhow::Context;
use rusqlite::{named_params, Row, Transaction};
use rusqlite_migration::{HookResult, Migrations, M};
use std::sync::Arc;
use tracing::info;

/// Get all DB migrations in history
pub fn migrations() -> Migrations<'static> {
    // There's no need for any down migrations here, because we have no
    // mechanism for going backwards
    Migrations::new(vec![
        M::up(
            // Path is the *canonicalzed* path to a collection file,
            // guaranteeing it will be stable and unique
            "CREATE TABLE collections (
                id              UUID PRIMARY KEY NOT NULL,
                path            BLOB NOT NULL UNIQUE
            )",
        ),
        M::up(
            // WARNING: this has been totally abolished by a later migration
            // The request state kind is a bit hard to map to tabular data.
            // Everything that we need to query on (HTTP status code,
            // end_time, etc.) is in its own column. The request/response
            // will be serialized into msgpack bytes
            "CREATE TABLE requests (
                id              UUID PRIMARY KEY NOT NULL,
                collection_id   UUID NOT NULL,
                profile_id      TEXT,
                recipe_id       TEXT NOT NULL,
                start_time      TEXT NOT NULL,
                end_time        TEXT NOT NULL,
                request         BLOB NOT NULL,
                response        BLOB NOT NULL,
                status_code     INTEGER NOT NULL,
                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
        ),
        M::up(
            // keys+values will be serialized as msgpack
            "CREATE TABLE ui_state (
                key             BLOB NOT NULL,
                collection_id   UUID NOT NULL,
                value           BLOB NOT NULL,
                PRIMARY KEY (key, collection_id),
                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
        ),
        // This is a sledgehammer migration. Added when we switch from
        // rmp_serde::to_vec to rmp_serde::to_vec_named. This affected the
        // serialization of all binary blobs, so there's no easy way to
        // migrate it all. It's easiest just to wipe it all out.
        M::up("DELETE FROM requests; DELETE FROM ui_state;"),
        // New table that flattens everything into its own column. This makes
        // it easy to browse data in the sqlite CLI, and gives better control
        // over migrations in the future if we add more fields.
        M::up_with_hook(
            "CREATE TABLE requests_v2 (
                id                  UUID PRIMARY KEY NOT NULL,
                collection_id       UUID NOT NULL,
                profile_id          TEXT,
                recipe_id           TEXT NOT NULL,
                start_time          TEXT NOT NULL,
                end_time            TEXT NOT NULL,

                method              TEXT NOT NULL,
                url                 TEXT_NOT NULL,
                request_headers     BLOB NOT NULL,
                request_body        BLOB,

                status_code         INTEGER NOT NULL,
                response_headers    BLOB NOT NULL,
                response_body       BLOB NOT NULL,

                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
            migrate_requests_v2_up,
        ),
        // UI state is now JSON encoded, instead of msgpack. This makes it
        // easier to browse, and the size payment should be minimal because
        // the key/value structure is simple
        M::up_with_hook(
            "CREATE TABLE ui_state_v2 (
                collection_id   UUID NOT NULL,
                key_type        TEXT NOT NULL,
                key             TEXT NOT NULL,
                value           TEXT NOT NULL,
                PRIMARY KEY (collection_id, key_type, key),
                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
            migrate_ui_state_v2_up,
        ),
    ])
}

/// Post-up hook to copy data from the `requests` table to `requests_v2`. This
/// will leave the old table around, so we can recover user data if something
/// goes wrong. We'll delete it in a later migration.
fn migrate_requests_v2_up(transaction: &Transaction) -> HookResult {
    fn load_exchange(
        row: &Row<'_>,
    ) -> Result<(CollectionId, Exchange), rusqlite::Error> {
        let collection_id = row.get("collection_id")?;
        let exchange = Exchange {
            id: row.get("id")?,
            start_time: row.get("start_time")?,
            end_time: row.get("end_time")?,
            // Deserialize from bytes
            request: Arc::new(row.get::<_, ByteEncoded<_>>("request")?.0),
            response: Arc::new(row.get::<_, ByteEncoded<_>>("response")?.0),
        };
        Ok((collection_id, exchange))
    }

    info!("Migrating table `requests` -> `requests_v2`");
    let mut select_stmt = transaction.prepare("SELECT * FROM requests")?;
    let mut insert_stmt = transaction.prepare(
        "INSERT INTO requests_v2 (
            id,
            collection_id,
            profile_id,
            recipe_id,
            start_time,
            end_time,
            method,
            url,
            request_headers,
            request_body,
            status_code,
            response_headers,
            response_body
        ) VALUES (
            :id,
            :collection_id,
            :profile_id,
            :recipe_id,
            :start_time,
            :end_time,
            :method,
            :url,
            :request_headers,
            :request_body,
            :status_code,
            :response_headers,
            :response_body
        )",
    )?;

    for result in select_stmt.query_map([], load_exchange)? {
        let Ok((collection_id, exchange)) = result
            .context("Error migrating from `requests` -> `requests_v2`")
            .traced()
        else {
            // Skip any conversions that fail so we don't kill everything
            continue;
        };

        info!(
            %collection_id,
            ?exchange,
            "Copying row from `requests` -> `requests_v2`",
        );
        insert_stmt.execute(named_params! {
            ":id": exchange.id,
            ":collection_id": collection_id,
            ":profile_id": &exchange.request.profile_id,
            ":recipe_id": &exchange.request.recipe_id,
            ":start_time": &exchange.start_time,
            ":end_time": &exchange.end_time,

            ":method": exchange.request.method.as_str(),
            ":url": exchange.request.url.as_str(),
            ":request_headers": SqlWrap(&exchange.request.headers),
            ":request_body": exchange.request.body.as_deref(),

            ":status_code": exchange.response.status.as_u16(),
            ":response_headers": SqlWrap(&exchange.response.headers),
            ":response_body": exchange.response.body.bytes(),
        })?;
    }

    Ok(())
}

/// Copy rows from ui_state -> ui_state_v2. Drop the old table since, unlike
/// requests, it's not a huge deal if we lose some data
fn migrate_ui_state_v2_up(transaction: &Transaction) -> HookResult {
    #[derive(Debug)]
    struct V1Row {
        collection_id: CollectionId,
        key_type: String,
        key: serde_json::Value,
        value: serde_json::Value,
    }

    fn load_row(row: &Row) -> Result<V1Row, rusqlite::Error> {
        // Key is encoded as a tuple of (type name, key)
        let ByteEncoded((key_type, key)): ByteEncoded<(
            String,
            serde_json::Value,
        )> = row.get("key")?;
        Ok(V1Row {
            collection_id: row.get("collection_id")?,
            key_type,
            key,
            value: row.get::<_, ByteEncoded<serde_json::Value>>("value")?.0,
        })
    }

    info!("Migrating table `ui_state` -> `ui_state_v2`");
    let mut select_stmt = transaction.prepare("SELECT * FROM ui_state")?;
    let mut insert_stmt = transaction.prepare(
        "INSERT INTO ui_state_v2 (collection_id, key_type, key, value)
        VALUES (:collection_id, :key_type, :key, :value)",
    )?;

    for result in select_stmt.query_map([], load_row)? {
        let Ok(row) = result
            .context("Error migrating from `ui_state` -> `ui_state_v2`")
            .traced()
        else {
            // Skip any conversions that fail so we don't kill everything
            continue;
        };

        info!(?row, "Copying row from `ui_state` -> `ui_state_v2`");
        insert_stmt.execute(named_params! {
            ":collection_id": row.collection_id,
            ":key_type": row.key_type,
            ":key": row.key.to_string(),
            ":value": row.value.to_string(),
        })?;
    }

    info!("Dropping table `ui_state`");
    transaction.execute("DROP TABLE ui_state", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::convert::{CollectionPath, JsonEncoded},
        http::{RequestRecord, ResponseRecord},
        test_util::Factory,
        util::get_repo_root,
    };
    use itertools::Itertools;
    use reqwest::{Method, StatusCode};
    use rstest::{fixture, rstest};
    use rusqlite::Connection;
    use serde_json::json;

    const MIGRATION_COLLECTIONS: usize = 1;
    const MIGRATION_ALL_V1: usize = 4;
    const MIGRATION_REQUESTS_V2: usize = MIGRATION_ALL_V1 + 1;
    const MIGRATION_UI_STATE_V2: usize = MIGRATION_REQUESTS_V2 + 1;

    #[fixture]
    fn connection() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations()
            .to_version(&mut connection, MIGRATION_COLLECTIONS)
            .unwrap();

        let collection_id = CollectionId::new();
        let collection_path: CollectionPath = get_repo_root()
            .join("slumber.yml")
            .as_path()
            .try_into()
            .unwrap();
        connection
            .execute(
                "INSERT INTO collections (id, path) VALUES (:id, :path)",
                named_params! {
                    ":id": collection_id,
                    ":path": collection_path,
                },
            )
            .unwrap();

        connection
    }

    /// Test copying data requests -> requests_v2
    #[rstest]
    fn test_migrate_requests_v2(mut connection: Connection) {
        let migrations = migrations();
        migrations
            .to_version(&mut connection, MIGRATION_ALL_V1)
            .unwrap();

        let exchanges = [
            Exchange::factory((
                RequestRecord {
                    method: Method::GET,
                    ..RequestRecord::factory(())
                },
                ResponseRecord::factory(StatusCode::NOT_FOUND),
            )),
            Exchange::factory((
                RequestRecord {
                    method: Method::POST,
                    ..RequestRecord::factory(())
                },
                ResponseRecord {
                    body: json!({"username": "ted"}).into(),
                    ..ResponseRecord::factory(StatusCode::CREATED)
                },
            )),
            Exchange::factory((
                RequestRecord {
                    method: Method::DELETE,
                    ..RequestRecord::factory(())
                },
                ResponseRecord::factory(StatusCode::BAD_REQUEST),
            )),
        ];
        for exchange in &exchanges {
            connection
                .execute(
                    "INSERT INTO
                        requests (
                            collection_id,
                            id,
                            profile_id,
                            recipe_id,
                            start_time,
                            end_time,
                            request,
                            response,
                            status_code
                        )
                        VALUES (
                            (SELECT id FROM collections),
                            :id, :profile_id, :recipe_id, :start_time,
                            :end_time, :request, :response, :status_code)",
                    named_params! {
                        ":id": exchange.id,
                        ":profile_id": &exchange.request.profile_id,
                        ":recipe_id": &exchange.request.recipe_id,
                        ":start_time": &exchange.start_time,
                        ":end_time": &exchange.end_time,
                        ":request": &ByteEncoded(&*exchange.request),
                        ":response": &ByteEncoded(&*exchange.response),
                        ":status_code": exchange.response.status.as_u16(),
                    },
                )
                .unwrap();
        }

        migrations
            .to_version(&mut connection, MIGRATION_REQUESTS_V2)
            .unwrap();

        // Make sure we didn't delete anything from the old table
        let count = connection
            .query_row("SELECT COUNT(*) FROM requests", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap();
        assert_eq!(count, exchanges.len());

        let mut stmt = connection.prepare("SELECT * FROM requests_v2").unwrap();
        let migrated: Vec<Exchange> = stmt
            .query_map::<Exchange, _, _>([], |row| row.try_into())
            .unwrap()
            .try_collect()
            .unwrap();
        assert_eq!(&migrated, &exchanges);
    }

    /// Test copying data ui_state -> ui_state_v2
    #[rstest]
    fn test_migrate_ui_state_v2(mut connection: Connection) {
        let migrations = migrations();
        migrations
            .to_version(&mut connection, MIGRATION_ALL_V1)
            .unwrap();

        let rows = [
            ("Scalar".to_owned(), json!(null), json!(3)),
            ("StringKey".to_owned(), json!("k1"), json!({"a": 1})),
            ("StringKey".to_owned(), json!("k2"), json!({"b": 2})),
            ("StringKey".to_owned(), json!("k3"), json!({"c": 3})),
            ("MapKey".to_owned(), json!({"key": "k1"}), json!([1, 2, 3])),
            ("MapKey".to_owned(), json!({"key": "k2"}), json!([4, 5, 6])),
            ("MapKey".to_owned(), json!({"key": "k3"}), json!([7, 8, 9])),
        ];

        for (key_type, key, value) in &rows {
            connection
                .execute(
                    "INSERT INTO
                        ui_state (collection_id, key, value)
                        VALUES ((SELECT id FROM collections), :key, :value)",
                    named_params! {
                        ":key": ByteEncoded((key_type, key)),
                        ":value": ByteEncoded(value),
                    },
                )
                .unwrap();
        }

        migrations
            .to_version(&mut connection, MIGRATION_UI_STATE_V2)
            .unwrap();

        // Make sure we dropped the old table
        let count = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                WHERE type = 'table' AND name = 'ui_state'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        assert_eq!(count, 0, "Expected `ui_state` table to be dropped");

        let mut stmt = connection.prepare("SELECT * FROM ui_state_v2").unwrap();
        let migrated: Vec<(String, serde_json::Value, serde_json::Value)> =
            stmt.query_map([], |row| {
                Ok((
                    row.get("key_type")?,
                    row.get::<_, JsonEncoded<_>>("key")?.0,
                    row.get::<_, JsonEncoded<_>>("value")?.0,
                ))
            })
            .unwrap()
            .try_collect()
            .unwrap();
        assert_eq!(&migrated, &rows);
    }
}
