# `slumber-mock`

An HTTP server that serves responses from Slumber's request history database.

This is useful for offline development or otherwise avoiding requests to remote servers.

For example, to load a mocked version of `https://shoal.lucaspickering.me/fish`:

```sh
curl http://localhost:5000/shoal.lucaspickering.me/fish
```

## Request Matching

Currently, matching is performed based solely on request host, method, and path. That means the following are **ignored**:

- URL fragment
- Query parameters
- Request headers
- Request body

If multiple requests in the database match an inbound request, the **most recent** response will be served.

## Installation

As this crate is very experimental, it is not published on crates.io. The only installation method is to clone this repo and install from source:

```sh
cargo install --path crates/mock
```
