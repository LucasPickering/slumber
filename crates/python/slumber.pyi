# Hand-written type stubs. Eventually pyo3 should be able to generate these
# https://pyo3.rs/v0.26.0/type-stub.html

class Collection:
    def __init__(self, path: str | None = None) -> None: ...
    async def request(
        self,
        recipe: str,
        profile: str | None = None,
        overrides: dict[str, str] = {},
        trigger: bool = True,
    ) -> "Response": ...
    def reload(self) -> None: ...

class Response:
    url: str
    status_code: int
    headers: dict[str, str]
    content: bytes
    text: str
    def raise_for_status(self) -> None: ...
