# JSON Schema: Completion & Validation

Slumber generates a publishes a [JSON Schema](https://json-schema.org/) for both the [Config](../api/configuration/index.md) and [Collection](../api/request_collection/index.md) formats. These are published via the [git repository](https://github.com/LucasPickering/slumber) and are accessible at:

- [config.json](https://raw.githubusercontent.com/LucasPickering/slumber/refs/tags/v{{#version}}/schemas/config.json)
- [collection.json](https://raw.githubusercontent.com/LucasPickering/slumber/refs/tags/v{{#version}}/schemas/collection.json)

> Replace `{{#version}}` with the version of Slumber you use for the most accurate schema definitions.

## IDE Completion

Most IDEs use [yaml-language-server](https://github.com/redhat-developer/yaml-language-server) for YAML highlighting and validation. This server supports additional validation with custom JSON schemas. To enable this, add this comment to the top of your config or collection file:

```yaml
# yaml-language-server: $schema=<url from above>
```
