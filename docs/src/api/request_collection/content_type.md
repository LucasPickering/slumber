# Content Type

Content type defines the various data formats that Slumber recognizes and can manipulate. Slumber is capable of displaying any text-based data format, but only specific formats support additional features such as [querying](../../user_guide/tui/filter_query.md) and formatting.

## Auto-detection

For chained requests, Slumber uses the [HTTP `Content-Type` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Type) to detect the content type. For chained files, it uses the file extension. For other [chain sources](./chain_source.md), or Slumber is unable to detect the content type, you'll have to manually provide the content type via the [chain](./chain.md) `content_type` field.

## Supported Content Types

| Content Type | HTTP Header        | File Extension(s) |
| ------------ | ------------------ | ----------------- |
| JSON         | `application/json` | `json`            |
