"""An in-memory S3 server for tests, run as a child process.

>>> import fakes3
>>> with fakes3.start() as server:
...     s3 = server.boto3_client()
...     s3.create_bucket(Bucket="demo")

With pytest, the ``fakes3_server`` fixture gives every test a fresh server.
"""

from ._control import Clock, Requests
from ._server import FakeS3, FakeS3Error, binary_path, start

__all__ = ["Clock", "FakeS3", "FakeS3Error", "Requests", "binary_path", "start"]
