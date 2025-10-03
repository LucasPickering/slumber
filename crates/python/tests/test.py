import json

import pytest
from pytest_httpserver import HTTPServer
from werkzeug import Request, Response

from slumber import Collection

COLLECTION_FILE = "tests/slumber.yml"


@pytest.fixture
def collection() -> Collection:
    return Collection(path=COLLECTION_FILE)


def echo(request: Request) -> Response:
    return Response(request.data)


async def test_request(collection: Collection, httpserver: HTTPServer) -> None:
    """Test a GET request"""

    expected_data = {"foo": "bar"}
    httpserver.expect_request("/get", method="GET").respond_with_json(
        expected_data
    )
    host = httpserver.url_for("")

    response = await collection.request("get", overrides={"host": host})
    data = json.loads(response.text)
    assert data == expected_data


async def test_profile_default(
    collection: Collection, httpserver: HTTPServer
) -> None:
    """Should use the default profile if none is given"""

    httpserver.expect_request("/post", method="POST").respond_with_handler(echo)
    host = httpserver.url_for("")

    response = await collection.request("post", overrides={"host": host})
    assert response.text == "Default"


async def test_profile(collection: Collection, httpserver: HTTPServer) -> None:
    """Change the profile"""

    httpserver.expect_request("/post", method="POST").respond_with_handler(echo)
    host = httpserver.url_for("")

    response = await collection.request(
        "post", profile="other", overrides={"host": host}
    )
    assert response.text == "Other"


async def test_trigger(collection: Collection, httpserver: HTTPServer) -> None:
    """Trigger an upstream request"""

    expected_data = {"foo": "bar"}
    httpserver.expect_request("/get", method="GET").respond_with_json(
        expected_data
    )
    httpserver.expect_request("/post", method="POST").respond_with_handler(echo)
    host = httpserver.url_for("")

    # `get` returns the JSON data, `trigger` sends that, and it gets echoed
    response = await collection.request("trigger", overrides={"host": host})
    data = json.loads(response.text)
    assert data == expected_data


async def test_trigger_disabled(collection: Collection) -> None:
    """Trigger an upstream request with triggers disabled"""

    with pytest.raises(match="Triggered request execution not allowed"):
        await collection.request("trigger", trigger=False)


async def test_raise_for_status(
    collection: Collection, httpserver: HTTPServer
) -> None:
    """Raise an exception with raise_for_status()"""
    httpserver.expect_request("/get", method="GET").respond_with_response(
        Response(status=400)
    )
    host = httpserver.url_for("")

    response = await collection.request("get", overrides={"host": host})
    with pytest.raises(ValueError) as exc:
        response.raise_for_status()
    assert str(exc.value) == "Status code 400 Bad Request"
