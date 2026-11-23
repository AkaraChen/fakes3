"""pytest fixtures for fakes3, registered automatically when the package is installed.

``fakes3_server``
    A fresh server for each test, stopped when the test ends (passed, failed or interrupted).
``fakes3_session_server``
    One server shared by the whole session (one per worker under pytest-xdist). Tests see each
    other's data unless they call ``reset()`` or ``restore()``.
``fakes3_options``
    Keyword arguments for :func:`fakes3.start` used by both. Override it in a ``conftest.py``.
"""

from typing import Any, Dict, Iterator

import pytest

from ._server import FakeS3, start


@pytest.fixture(scope="session")
def fakes3_options() -> Dict[str, Any]:
    """Options for the servers the fixtures start; override to change them, e.g.
    ``return {"bucket_defaults": "aws"}``."""
    return {}


@pytest.fixture
def fakes3_server(fakes3_options: Dict[str, Any]) -> Iterator[FakeS3]:
    """A fresh fakes3 server for this test."""
    with start(**fakes3_options) as server:
        yield server


@pytest.fixture(scope="session")
def fakes3_session_server(fakes3_options: Dict[str, Any]) -> Iterator[FakeS3]:
    """One fakes3 server for the whole test session."""
    with start(**fakes3_options) as server:
        yield server
