"""fakes3 with boto3, through the pytest fixtures."""

from datetime import datetime, timedelta, timezone
import urllib.error
import urllib.request

import pytest
from botocore.exceptions import ClientError

import fakes3

# Plain HTTP to 127.0.0.1, whatever proxy the environment configures.
_http = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http_get(url):
    try:
        with _http.open(url, timeout=10) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


@pytest.mark.parametrize("run", [1, 2])
def test_each_test_gets_a_fresh_server(fakes3_server, run):
    s3 = fakes3_server.boto3_client()
    # Whichever of the two runs first, the other one still starts empty.
    assert s3.list_buckets()["Buckets"] == []
    s3.create_bucket(Bucket="shared-name")
    s3.put_object(Bucket="shared-name", Key="run", Body=str(run).encode())
    assert fakes3_server.requests.count() == 3


def test_connection_details(fakes3_server):
    assert fakes3_server.endpoint == f"http://127.0.0.1:{fakes3_server.port}"
    assert fakes3_server.host == "127.0.0.1"
    assert fakes3_server.port > 0
    assert fakes3_server.access_key == "fakes3"
    assert fakes3_server.secret_key == "fakes3secret"
    assert fakes3_server.running


def test_put_get_with_plain_boto3(fakes3_server):
    import boto3

    s3 = boto3.client(
        "s3",
        endpoint_url=fakes3_server.endpoint,
        aws_access_key_id=fakes3_server.access_key,
        aws_secret_access_key=fakes3_server.secret_key,
        region_name="us-east-1",
    )
    s3.create_bucket(Bucket="demo")
    s3.put_object(Bucket="demo", Key="hello.txt", Body=b"hi")
    assert s3.get_object(Bucket="demo", Key="hello.txt")["Body"].read() == b"hi"


def test_presigned_url_expires_on_the_virtual_clock(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="share")
    s3.put_object(Bucket="share", Key="report.csv", Body=b"a,b\n1,2\n")
    url = s3.generate_presigned_url(
        "get_object", Params={"Bucket": "share", "Key": "report.csv"}, ExpiresIn=60
    )

    assert http_get(url) == (200, b"a,b\n1,2\n")

    fakes3_server.clock.advance("2m")
    status, body = http_get(url)
    assert status == 403
    assert b"AccessDenied" in body and b"Request has expired" in body

    # Back on the host clock, the URL is valid again.
    fakes3_server.clock.reset()
    assert http_get(url)[0] == 200


def test_request_log_counts_put_object(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="logs")
    for i in range(3):
        s3.put_object(Bucket="logs", Key=f"day/{i}.log", Body=b"x" * i)
    s3.get_object(Bucket="logs", Key="day/1.log")
    with pytest.raises(ClientError):
        s3.get_object(Bucket="logs", Key="missing")

    requests = fakes3_server.requests
    assert requests.count(operation="PutObject") == 3
    puts = requests.list(operation="PutObject")
    assert [r["key"] for r in puts] == ["day/0.log", "day/1.log", "day/2.log"]
    assert [r["bytes_in"] for r in puts] == [0, 1, 2]
    assert all(r["bucket"] == "logs" and r["status"] == 200 for r in puts)
    assert [r["operation"] for r in requests.list(limit=2)] == ["GetObject", "GetObject"]
    errors = requests.list(status="error")
    assert [(r["key"], r["status"]) for r in errors] == [("missing", 404)]
    assert requests.count(status=404) == 1
    assert requests.count(key_prefix="day/", method="GET") == 1

    last = requests.list(limit=1)[0]["seq"]
    s3.head_object(Bucket="logs", Key="day/2.log")
    assert [r["operation"] for r in requests.list(after=last)] == ["HeadObject"]

    requests.clear()
    assert requests.count() == 0
    assert requests.configure(capacity=2)["capacity"] == 2
    assert requests.configure(enabled=False)["enabled"] is False
    s3.list_buckets()
    assert requests.count() == 0
    assert requests.config() == {"enabled": False, "capacity": 2, "stored": 0, "dropped": 0}


def test_snapshot_and_restore(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="data")
    s3.put_object(Bucket="data", Key="a", Body=b"original")
    info = fakes3_server.snapshot("seeded")
    assert info["name"] == "seeded" and info["buckets"] == 1 and info["objects"] == 1

    s3.put_object(Bucket="data", Key="a", Body=b"changed")
    s3.put_object(Bucket="data", Key="b", Body=b"new")
    s3.create_bucket(Bucket="later")

    fakes3_server.restore("seeded")
    assert s3.get_object(Bucket="data", Key="a")["Body"].read() == b"original"
    keys = [o["Key"] for o in s3.list_objects_v2(Bucket="data")["Contents"]]
    assert keys == ["a"]
    assert [b["Name"] for b in s3.list_buckets()["Buckets"]] == ["data"]

    # A snapshot can be restored again and again.
    s3.delete_object(Bucket="data", Key="a")
    fakes3_server.restore("seeded")
    assert s3.get_object(Bucket="data", Key="a")["Body"].read() == b"original"

    assert [s["name"] for s in fakes3_server.snapshots()] == ["seeded"]
    unnamed = fakes3_server.snapshot()
    assert unnamed["name"].startswith("snap-")
    fakes3_server.delete_snapshot(unnamed["name"])
    assert [s["name"] for s in fakes3_server.snapshots()] == ["seeded"]

    with pytest.raises(fakes3.FakeS3Error) as e:
        fakes3_server.restore("nope")
    assert e.value.code == "NotFound" and e.value.status == 404
    with pytest.raises(fakes3.FakeS3Error) as e:
        fakes3_server.snapshot("seeded")
    assert e.value.status == 409


def test_reset(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="data")
    fakes3_server.snapshot("base")
    fakes3_server.clock.freeze()

    fakes3_server.reset()
    assert s3.list_buckets()["Buckets"] == []
    assert not fakes3_server.clock.is_shifted()
    # The request log was cleared before ListBuckets.
    assert fakes3_server.requests.count() == 1
    assert len(fakes3_server.snapshots()) == 1

    fakes3_server.reset(snapshots=True)
    assert fakes3_server.snapshots() == []


def test_clock(fakes3_server):
    clock = fakes3_server.clock
    assert not clock.is_frozen() and not clock.is_shifted()

    at = datetime(2030, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    assert clock.freeze(at) == at
    assert clock.is_frozen() and clock.is_shifted()
    assert clock.advance(timedelta(days=1, milliseconds=500)) == at + timedelta(days=1, milliseconds=500)
    assert clock.advance(-0.5) == at + timedelta(days=1)
    assert clock.advance("-1d") == at
    assert clock.now() == at
    assert clock.set("2031-06-01T00:00:00Z") == datetime(2031, 6, 1, tzinfo=timezone.utc)
    # A naive datetime is UTC.
    assert clock.set(datetime(2032, 1, 1)) == datetime(2032, 1, 1, tzinfo=timezone.utc)

    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="times")
    s3.put_object(Bucket="times", Key="k", Body=b"")
    assert s3.head_object(Bucket="times", Key="k")["LastModified"] == datetime(2032, 1, 1, tzinfo=timezone.utc)

    clock.unfreeze()
    assert not clock.is_frozen() and clock.is_shifted()
    clock.reset()
    assert not clock.is_shifted()
    assert abs(clock.now() - datetime.now(timezone.utc)) < timedelta(minutes=1)


def test_stats_and_status(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="mem")
    s3.put_object(Bucket="mem", Key="k", Body=b"x" * 1000)
    assert fakes3_server.memory()["used"] >= 1000
    assert fakes3_server.stats()["store"]["objects"] == 1
    status = fakes3_server.status()
    assert status["auth"] == "any-credentials"


def test_session_server_is_shared(fakes3_session_server, request):
    s3 = fakes3_session_server.boto3_client()
    name = f"session-{request.node.name}".lower().replace("_", "-")[:63]
    s3.create_bucket(Bucket=name)
    assert name in [b["Name"] for b in s3.list_buckets()["Buckets"]]


def test_session_server_is_the_same_object(fakes3_session_server, request):
    assert fakes3_session_server is request.getfixturevalue("fakes3_session_server")
    assert fakes3_session_server.running
