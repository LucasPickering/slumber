//! TODO

use rusqlite::Transaction;
use rusqlite_migration::{HookResult, Migrations, M};

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
            // The request state kind is a bit hard to map to tabular data.
            // Everything that we need to query on (HTTP status code,
            // end_time, etc.) is in its own column. Therequest/response
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
    ])
}
