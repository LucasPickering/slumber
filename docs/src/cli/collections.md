# `slumber collections`

View and manipulate stored collection history/state. Slumber uses a local database to store all request/response history, as well as UI state and other persisted values. **As a user, you rarely have to worry about this.** The most common scenario in which you _do_ have to is if you've renamed a collection file and want to migrate the history to match the new path.

See `slumber collections --help` for more options.

## History & Migration

Each collection needs a unique ID, which generated when the collection is first loaded by Slumber and bound to the collection file's path. This ID is used to persist request history and other data related to the collection. If you move a collection file, a new ID will be generated and it will be unlinked from its previous history. If you want to retain that history, you can migrate data from the old ID to the new one like so:

```sh
slumber collections migrate slumber-old.yml slumber-new.yml
```

If you don't remember the path of the old file, you can list all known collections with:

```sh
slumber collections list
```
