# Python

Slumber provides a native [Python package](https://pypi.org/project/slumber-python/) to load and use your Slumber collections. This makes it very easy to write Python scripts that make requests based on your collection.

To install:

```sh
pip install slumber-python
```

## Examples

### Sending a Request

By default, the library loads the same collection file that the CLI/TUI would, [according to these rules](../api/request_collection/index.html#format--loading).

```py
from slumber import Collection

collection = Collection()
response = collection.request('example_get')
print(response.context) # Response body as bytes
print(response.text) # Response body as a str
```

### Load Different Collection

You can specify which collection file should be loaded:

```py
from slumber import Collection

# You can specify a specific file:
collection = Collection(path="./other-collection.yml")
# Or a directory, in which case the auto-load rules will apply in that dir
collection = Collection(path="./my-collections/")
```

### JSON

```py
import json
from slumber import Collection

collection = Collection()
response = collection.request('example_get')
data = json.loads(response.text)
```

### Check Status Code

By default, Slumber will _not_ raise an error for 4xx/5xx status codes, only if the request/response fails to transmit. To check the status code, use `raise_for_status()`:

```py
from slumber import Collection

collection = Collection()
response = collection.request('example_get')
response.raise_for_status()
```
