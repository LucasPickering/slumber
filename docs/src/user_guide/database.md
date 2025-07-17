# Database & Persistence

> Note: This is an advanced feature. The vast majority of users can use Slumber all they want without even knowing the database exists.

Slumber uses a [SQLite](https://www.sqlite.org/) database to persist requests and responses. The database also stores UI state that needs to be persisted between sessions. This database exists exclusively on your device. **The Slumber database is never uploaded to the cloud or shared in any way.** Slumber does not make any network connections beyond the ones you define and execute yourself. You own your data; I don't want it.

Data for all your Slumber collections are stored in a single file. To find this file, run `slumber show paths`, and look for the `Database` entry. I encourage you to browse this file if you're curious; it's pretty simple and there's nothing secret in it. Keep in mind though that **the database format is NOT considered part of Slumber's API contract.** It may change at any time, including the database path moving or tables be changed or removed, even in a minor or patch release.

### Controlling Persistence

By default, all requests made in the TUI are stored in the database. This enables the history browser, allowing you to browse past requests. While generally useful, this may not be desired in all cases. However, there are some cases where you may not want requests persisted:

- The request or response may contain sensitive data
- The response is very large and impacts app performance

You can disable persistence for a single recipe by setting `persist: false` [for that recipe](../api/request_collection/request_recipe.md#recipe-fields). You can disable history globally by setting `persist: false` in the [global config file](../api/configuration/index.md). Note that this only disables _request_ persistence. UI state, such as selection state for panes and checkboxes, is still written to the database.

> **NOTE:** Disabling persistence does _not_ delete existing request history. [See here](#deleting-request-history) for how to do that.

Slumber will generally continue to work just fine with request persistence disabled. Requests and responses are still cached in memory, they just aren't written to the database anymore and therefore can't be recovered after the current session is closed. If you disable persistence, you will notice a few impacts on functionality:

- The history modal will only show requests made during the current session
- Chained requests can only access responses from the current session. [Consider adding `trigger="no_history"` to the `response` call](../api/template_functions.md#response) to automatically refetch it on new sessions.

Unlike the TUI, requests made from the CLI are _not_ persisted by default. This is because the CLI is often used for scripting and bulk requests. Persisting these requests could have major performance impacts for little to no practical gain. Pass the `--persist` flag to `slumber request` to persist a CLI request.

### Deleting Request History

There are a few ways to delete requests from history:

- In the TUI. Open the actions menu while a request/response is selected to delete that request. From the recipe list/recipe pane, you can delete all requests for that recipe.
- `slumber history delete` can delete one or more commands at a time. Combine with `slumber history list` for bulk deletes: `slumber history list login --id-only | xargs slumber history delete`
- `slumber collections delete` can delete all history for a single collection. If you have an old collection that you no longer use, you can delete it from the list using this command. **Note:** If you moved a collection file and want to remove the old file's history, you can also [migrate the history to the new file location](#migrating-collections).
- Manually modifying the database. You can access the DB with `slumber db`. While this is not an officially supported technique (as the DB schema may change without warning), it's simple enough to navigate if you want to performance bulk deletes with custom criteria.

### Migrating Collections

As all Slumber collections' histories are stored in the same SQLite database, each collection gets a unique [UUID](https://en.wikipedia.org/wiki/Universally_unique_identifier) generated when it is first accessed. This UUID is used to persist request history and other data related to the collection. This UUID is bound to the collection's path. If you move a collection file, a new UUID will be generated and it will be unlinked from its previous history. If you want to retain that history, you can migrate data from the old ID to the new one like so:

```sh
slumber collections migrate slumber-old.yml slumber-new.yml
```

If you don't remember the path of the old file, you can list all known collections with:

```sh
slumber collections list
```
