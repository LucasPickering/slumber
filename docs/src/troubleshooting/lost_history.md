# Lost Request History

If you've lost your request history, there are a few possible causes. In most cases request history is non-essential, but if you really want it back there may be a fix available.

If none of these fixes worked for you, and you still want your request history back, please [open an issue](https://github.com/LucasPickering/slumber/issues/new) and provide as much detail as possible.

## Moved Collection File

If history is lost for an entire collection, the most likely cause is that you moved your collection file. To fix this, you can [migrate your request history](../api/request_collection/index.md#collection-history--migration).

## Changed Recipe ID

If you've lost request history for just a single recipe, you likely changed the recipe ID, which is the key associated with the recipe in your collection file (the parent folder(s) do **not** affect this). Unfortunately currently the only way to fix this is to revert to the old recipe ID.

## Wrong Profile or Changed Profile ID

Each request+response in history is associated with a specific profile. If you're not seeing your expected request history, you may have a different profile selected than the one used to send the request(s).

Alternatively, you may have changed the ID of the associated profile. If so, unfortunately the only way to fix this is to revert to the old profile ID.
