# Content Type

Content type defines the various data formats that Slumber recognizes and can manipulate. Slumber is capable of displaying any text-based data format, but only specific formats support additional features such as [querying](../../user_guide/tui/filter_query.md) and formatting.

Slumber uses the `Content-Type` header to determine the format of a request/response.

## Supported Content Types

| Content Type | HTTP Header        |
| ------------ | ------------------ |
| JSON         | `application/json` |
