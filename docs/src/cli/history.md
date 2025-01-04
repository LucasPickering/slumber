# `slumber history`

View your Slumber request history.

## Examples

```sh
slumber history list login # List all requests for the "login" recipe
slumber history list login -p dev # List all requests for "login" under the "dev" profile
slumber history get login # Get the most recent request/response for "login"
slumber history get 548ba3e7-3b96-4695-9856-236626ea0495 # Get a particular request/response by ID (IDs can be retrieved from the `list` subcommand)
```
