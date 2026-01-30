use crate::util::confirm;
use rusqlite::Transaction;
use rusqlite_migration::{HookError, HookResult, M, Migrations};

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
            migrate_requests_v2,
        ),
        // UI state is now JSON encoded, instead of msgpack. This makes it
        // easier to browse, and the size payment should be minimal because
        // the key/value structure is simple
        M::up(
            "CREATE TABLE ui_state_v2 (
                collection_id   UUID NOT NULL,
                key_type        TEXT NOT NULL,
                key             TEXT NOT NULL,
                value           TEXT NOT NULL,
                PRIMARY KEY (collection_id, key_type, key),
                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
        ),
        // v3.0 - Old tables are gone entirely. For new DBs we create then drop
        // these tables which is a waste, but it's necessary so new  See
        // migrate_v3 for more info.
        M::up("DROP TABLE IF EXISTS requests; DROP TABLE IF EXISTS ui_state"),
        M::up(
            // reqwest uses HTTP/1.1 by default, so we know all old requests
            // are of that version
            "ALTER TABLE requests_v2 ADD COLUMN \
            http_version TEXT NOT NULL DEFAULT 'HTTP/1.1'",
        )
        .down("ALTER TABLE requests_v2 DROP COLUMN http_version"),
        M::up("ALTER TABLE collections ADD COLUMN name TEXT")
            .down("ALTER TABLE collections DROP COLUMN name"),
        M::up(
            // Store query/export commands. The purpose of this is to suggest
            // commands from history, so there's no reason to store duplicates.
            // The time is when it was most recently run
            "CREATE TABLE commands (
                collection_id   UUID NOT NULL,
                command         TEXT NOT NULL,
                time            TEXT NOT NULL,

                PRIMARY KEY (collection_id, command),
                FOREIGN KEY(collection_id) REFERENCES collections(id)
            )",
        )
        .down("DROP TABLE IF EXISTS commands"),
        // Add a column that holds the variant of RequestBody. Each variant has
        // a u8 code associated with it. It'd be nice to have a CHECK constraint
        // here to ensure request_body is only populated if kind=1, but sqlite
        // doesn't support adding CHECK constraints to existing tables
        M::up(
            "ALTER TABLE requests_v2 \
                ADD COLUMN request_body_kind INTEGER NOT NULL DEFAULT 0;
            UPDATE requests_v2 SET request_body_kind = request_body IS NOT NULL",
        )
        .down("ALTER TABLE requests_v2 DROP COLUMN request_body_kind"),
    ])
}

/// Migrate to the requests_v2 table. In Slumber v1.8.0, we completely
/// changed the schema for the requests and ui_state tables, hence the tables
/// requests_v2 and ui_state_v2. For versions >=1.8.0,<3.0.0 this migration
/// would copy over data from the old table to the new one. For versions
/// \>=3.0.0, that copy has been removed because it involved a lot of code and a
/// dependency on rmp-serde. Users cannot upgrade from <1.8.0 directly to
/// \>=3.0.0; they'll need to go to something in between (probably the final 2.x
/// version) first.
///
/// See <https://github.com/LucasPickering/slumber/issues/306> for more info
fn migrate_requests_v2(transaction: &Transaction) -> HookResult {
    // There are 3 possible scenarios to cover:
    // 1. Fresh DB - user is starting Slumber for the first time on >=3.0.0
    //   - (or, they've run Slumber before but never made requests)
    //   - We'll run this migration but the `requests` table will be empty
    // 2. Upgrading from >=1.8.0,<3.0.0 to >=3.0.0
    //   - This migration will not run in this case, because it already ran on
    //     the old version
    // 3. Upgrading from <1.8.0 to >=3.0.0
    //   - Ask the user if they want to preserve their old data. If so, they'll
    //     need to do an intermediate upgrade
    let old_requests_count =
        transaction.query_row("SELECT COUNT(*) FROM requests", (), |row| {
            row.get::<_, u32>(0)
        })?;
    if old_requests_count > 0 {
        let delete = confirm(
            "You are upgrading from Slumber <1.8.0 to Slumber >=3.0.0. \
                Your request history database contains old requests that \
                cannot be migrated directly to a newer format. You can proceed \
                with the upgrade by DELETING THE OLD REQUESTS now, or you can \
                retain the requests by upgrading to an intermediate version \
                first.\nWould you like to DELETE YOUR REQUEST HISTORY?",
        );
        if delete {
            // We can just proceed and a future migration will drop the old
            // table
            Ok(())
        } else {
            Err(HookError::Hook(
                "Migration aborted. Upgrade to a version earlier than 3.0.0 \
                first to retain your request history."
                    .into(),
            ))
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        collection::RecipeId, database::CollectionId, http::RequestId,
    };
    use chrono::Utc;
    use rusqlite::{Connection, named_params};
    use slumber_util::Factory;

    /// Test migrating a fresh DB works. The most basic and shitty of tests!
    #[test]
    fn test_migrate_latest() {
        let mut connection = Connection::open_in_memory().unwrap();
        let migrations = migrations();
        migrations.to_latest(&mut connection).unwrap();
        let request_count = connection
            .query_row("SELECT COUNT(*) FROM requests_v2", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap();
        assert_eq!(request_count, 0);
    }

    /// Test the migration that added the `request_body_kind` column. Any
    /// existing request with a body will be kind `Some`. Anything else will
    /// be kind `None`. This means stream/large bodies will instead look like
    /// missing bodies. No way to distinguish them though (which is why the new
    /// column was added).
    #[test]
    fn test_migrate_request_body_kind() {
        fn insert_exchange(
            connection: &Connection,
            request_body: Option<&[u8]>,
        ) {
            connection
                .execute(
                    "INSERT INTO
                    requests_v2 (
                        id,
                        collection_id,
                        profile_id,
                        recipe_id,
                        start_time,
                        end_time,
                        http_version,
                        method,
                        url,
                        request_headers,
                        request_body,
                        status_code,
                        response_headers,
                        response_body
                    )
                    VALUES (
                        :id,
                        :collection_id,
                        :profile_id,
                        :recipe_id,
                        :start_time,
                        :end_time,
                        :http_version,
                        :method,
                        :url,
                        :request_headers,
                        :request_body,
                        :status_code,
                        :response_headers,
                        :response_body
                    )",
                    named_params! {
                        ":id": RequestId::new(),
                        ":collection_id": CollectionId::new(),
                        ":profile_id": "",
                        ":recipe_id": RecipeId::factory(()),
                        ":start_time": Utc::now(),
                        ":end_time": Utc::now(),

                        ":http_version": "1.1",
                        ":method": "POST",
                        ":url": "http://localhost",
                        ":request_headers": b"",
                        ":request_body": request_body,

                        ":status_code": 200,
                        ":response_headers": b"",
                        ":response_body": b"",
                    },
                )
                .unwrap();
        }

        let mut connection = Connection::open_in_memory().unwrap();
        let migrations = migrations();

        // Migrate to the version before
        migrations.to_version(&mut connection, 10).unwrap();
        // Make sure we got the right version
        let columns: Vec<String> = connection
            .prepare("PRAGMA table_info(requests_v2)")
            .unwrap()
            .query_map((), |row| row.get::<_, String>("name"))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        assert!(!columns.contains(&"request_body_kind".to_owned()));

        // Disable FK checks to make the insertions easier
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        // Add a few different exchanges
        insert_exchange(&connection, None); // No body
        insert_exchange(&connection, Some(b"")); // Empty body
        insert_exchange(&connection, Some(b"data")); // With body

        // Do the migration
        migrations.to_version(&mut connection, 11).unwrap();

        let bodies = connection
            .prepare(
                "SELECT request_body_kind, request_body FROM requests_v2 \
                ORDER BY start_time",
            )
            .unwrap()
            .query_map((), |row| {
                let kind: u8 = row.get("request_body_kind")?;
                let body: Option<Vec<u8>> = row.get("request_body")?;
                Ok((kind, body))
            })
            .unwrap()
            .collect::<Result<Vec<(u8, Option<Vec<u8>>)>, _>>()
            .unwrap();
        assert_eq!(
            bodies,
            vec![
                (0, None),
                (1, Some(b"".to_vec())),
                (1, Some(b"data".to_vec()))
            ]
        );
    }
}
