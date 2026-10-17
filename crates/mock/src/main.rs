use axum::{
    Router,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::any,
};
use clap::Parser;
use itertools::Itertools;
use reqwest::StatusCode;
use slumber_core::{
    collection::CollectionFile,
    database::{CollectionDatabase, Database, DatabaseError},
};
use std::{io, path::PathBuf, sync::Arc};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info, level_filters::LevelFilter, trace};
use tracing_subscriber::{
    Layer, fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt,
};

/// Arguments that are available to all subcommands and the TUI
#[derive(Debug, Parser)]
pub struct Args {
    /// Collection file, which defines profiles, recipes, etc.
    ///
    /// If omitted, check the current and all parent directories for the
    /// following files (in this order): slumber.yml, slumber.yaml,
    /// .slumber.yml, .slumber.yaml. If a directory is passed, apply the
    /// same search logic from the given directory rather than the current.
    #[clap(long, short)]
    pub file: Option<PathBuf>,

    /// Host to bind to
    #[clap(long, default_value = "localhost")]
    pub host: String,

    /// Port to host on
    #[clap(long, default_value_t = 5000)]
    pub port: u16,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    initialize_tracing();
    let collection_file = CollectionFile::new(args.file)?;
    let database = Database::load()?.into_collection(&collection_file)?;

    // Fetch the initial list of hosts for logging
    let all_requests = database.get_all_requests()?;
    let hosts = all_requests
        .iter()
        .filter_map(|(_, _, url)| url.host_str())
        .unique()
        .sorted()
        .collect_vec();
    info!(%collection_file, ?hosts, "Mocking requests");

    let app = Router::new()
        .route("/{host}/{*path}", any(mock))
        .fallback(|| async { HttpError::NotFound })
        .with_state(database)
        .layer(TraceLayer::new_for_http());

    let host = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&host).await?;
    info!("Listening on http://{host}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve a mocked request from the database
///
/// The path should be `/{host}/{path}`
async fn mock(
    State(database): State<CollectionDatabase>,
    method: axum::http::Method,
    Path((host, mut path)): Path<(String, String)>,
) -> Result<Response, HttpError> {
    path.insert(0, '/'); // The path lacks the initial / needed for comparison
    let requests = database.get_all_requests()?;
    let id = requests
        .iter()
        .find_map(|(id, req_method, req_url)| {
            // Neither path will include query params; ignoring those for now
            if *req_method == method
                && req_url.host_str() == Some(&host)
                && req_url.path() == path
            {
                info!("Matched request {id}");
                Some(id)
            } else {
                trace!("Did not match request {id} {req_method} {req_url}");
                None
            }
        })
        .ok_or_else(|| {
            error!("{method} {host}/{path} did not match any requests");
            HttpError::NotFound
        })?;

    // Found a matching request. Grab its response and return it.
    //
    // This could be faster with another method that fetches just the response.
    // Skipping that for now though.
    let exchange = database
        .get_exchange(*id)?
        // 404 *should* be impossible here since we just got the request ID
        // from the DB. This error *could* be better but I'm being lazy.
        .ok_or_else(|| HttpError::NotFound)?;
    // We haven't shared the response anywhere, so it should be exclusive
    let mut response = Arc::into_inner(exchange.response)
        .expect("Expected exclusive access to response");

    // This is a point-to-point header, not meant to be the same across the
    // whole response. Including it can cause the response to be empty.
    response.headers.remove("transfer-encoding");

    Ok((
        response.status,
        response.headers,
        response.body.into_bytes(),
    )
        .into_response())
}

#[derive(Debug)]
enum HttpError {
    /// Error accessing database
    Database(DatabaseError),
    /// Mock wasn't found
    NotFound,
}

impl From<DatabaseError> for HttpError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Database(error) => {
                error!(%error, "Database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            Self::NotFound => (StatusCode::NOT_FOUND, "No matching request"),
        };
        (status, message).into_response()
    }
}

/// Initialize stderr tracing
fn initialize_tracing() {
    // Enable console output for CLI. By default logging is off, but it can be
    // turned up with the verbose flag
    let stderr_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_target(true)
        .with_span_events(FmtSpan::NONE)
        .without_time()
        .with_filter(LevelFilter::TRACE);

    tracing_subscriber::registry()
        .with(stderr_subscriber)
        .init();
}
