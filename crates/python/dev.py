import asyncio

from slumber import Collection

collection = Collection()
response = asyncio.run(collection.request("new_session"))
