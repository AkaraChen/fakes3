//! Responses must be byte-identical whether or not large bodies go out with `sendfile`.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Conn {
    s: TcpStream,
    buf: Vec<u8>,
}

struct Resp {
    head: String,
    body: Vec<u8>,
}

impl Resp {
    /// The head without headers that legitimately differ between servers or runs.
    fn stable_head(&self) -> String {
        self.head
            .lines()
            .filter(|l| {
                let l = l.to_ascii_lowercase();
                !l.starts_with("date:") && !l.starts_with("last-modified:") && !l.starts_with("x-amz-request-id")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then_some(v.trim())
        })
    }
}

impl Conn {
    async fn open(addr: std::net::SocketAddr) -> Self {
        Self {
            s: TcpStream::connect(addr).await.unwrap(),
            buf: Vec::new(),
        }
    }

    async fn send(&mut self, method: &str, path: &str, extra: &[&str], body: &[u8]) {
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n", body.len());
        for h in extra {
            req += &format!("{h}\r\n");
        }
        req += "\r\n";
        self.s.write_all(req.as_bytes()).await.unwrap();
        self.s.write_all(body).await.unwrap();
    }

    async fn recv(&mut self) -> Resp {
        loop {
            if let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8(self.buf[..end].to_vec()).unwrap();
                let mut resp = Resp { head, body: Vec::new() };
                let len: usize = resp.header("content-length").map_or(0, |v| v.parse().unwrap());
                let total = end + 4 + len;
                while self.buf.len() < total {
                    self.fill().await;
                }
                resp.body = self.buf[end + 4..total].to_vec();
                self.buf.drain(..total);
                return resp;
            }
            self.fill().await;
        }
    }

    async fn fill(&mut self) {
        let mut chunk = vec![0u8; 1 << 20];
        let n = self.s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed");
        self.buf.extend_from_slice(&chunk[..n]);
    }

    async fn call(&mut self, method: &str, path: &str, extra: &[&str], body: &[u8]) -> Resp {
        self.send(method, path, extra, body).await;
        self.recv().await
    }
}

fn pattern(n: usize, seed: u32) -> Vec<u8> {
    (0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) ^ seed) as u8).collect()
}

/// Loads the same objects into a server and returns every response a client can see for them.
async fn exercise(sendfile: bool) -> Vec<Resp> {
    let server = fakes3::Server::start(fakes3::Config {
        auth: false,
        sendfile,
        ..Default::default()
    })
    .await
    .unwrap();
    let mut c = Conn::open(server.addr()).await;
    let r = c.call("PUT", "/bkt", &[], b"").await;
    assert_eq!(
        r.head.lines().next().unwrap(),
        "HTTP/1.1 200 OK",
        "{}",
        String::from_utf8_lossy(&r.body)
    );

    let big = pattern(6 << 20, 1);
    c.call("PUT", "/bkt/big", &["Content-Type: application/x-test"], &big).await;
    c.call("PUT", "/bkt/small", &[], &pattern(1000, 2)).await;

    // A multipart object: two memfd parts and a heap tail.
    let r = c.call("POST", "/bkt/mp?uploads", &[], b"").await;
    let body = String::from_utf8(r.body).unwrap();
    let id = body
        .split("<UploadId>")
        .nth(1)
        .unwrap()
        .split("</UploadId>")
        .next()
        .unwrap()
        .to_owned();
    let parts = [pattern(5 << 20, 3), pattern(5 << 20, 4), pattern(777, 5)];
    let mut complete = String::from("<CompleteMultipartUpload>");
    for (i, p) in parts.iter().enumerate() {
        let r = c.call("PUT", &format!("/bkt/mp?partNumber={}&uploadId={id}", i + 1), &[], p).await;
        let etag = r.header("etag").unwrap();
        complete += &format!("<Part><PartNumber>{}</PartNumber><ETag>{etag}</ETag></Part>", i + 1);
    }
    complete += "</CompleteMultipartUpload>";
    let r = c.call("POST", &format!("/bkt/mp?uploadId={id}"), &[], complete.as_bytes()).await;
    assert!(r.head.starts_with("HTTP/1.1 200"), "{}", r.head);
    let mp: Vec<u8> = parts.concat();

    let mut out = Vec::new();
    fn check(out: &mut Vec<Resp>, r: Resp, want: &[u8]) {
        assert!(r.body == want, "body mismatch ({} vs {} bytes)", r.body.len(), want.len());
        out.push(r);
    }

    check(&mut out, c.call("GET", "/bkt/big", &[], b"").await, &big);
    check(&mut out, c.call("GET", "/bkt/small", &[], b"").await, &pattern(1000, 2));
    check(&mut out, c.call("GET", "/bkt/mp", &[], b"").await, &mp);
    check(
        &mut out,
        c.call("GET", "/bkt/big", &["Range: bytes=1000-4999999"], b"").await,
        &big[1000..5_000_000],
    );
    check(
        &mut out,
        c.call("GET", "/bkt/big", &["Range: bytes=-300000"], b"").await,
        &big[big.len() - 300_000..],
    );
    check(&mut out, c.call("GET", "/bkt/big", &["Range: bytes=5-9"], b"").await, &big[5..10]);
    // Range spanning the memfd parts and the heap tail of the multipart object.
    check(
        &mut out,
        c.call("GET", "/bkt/mp", &["Range: bytes=5000000-10486000"], b"").await,
        &mp[5_000_000..=10_486_000],
    );
    check(&mut out, c.call("GET", "/bkt/mp?partNumber=2", &[], b"").await, &parts[1]);
    check(&mut out, c.call("GET", "/bkt/mp?partNumber=3", &[], b"").await, &parts[2]);

    // Pipelined requests on one connection, with an error in between.
    c.send("GET", "/bkt/big", &[], b"").await;
    c.send("GET", "/bkt/missing", &[], b"").await;
    c.send("GET", "/bkt/mp", &["Range: bytes=1-3000000"], b"").await;
    c.send("GET", "/bkt/big", &[], b"").await;
    check(&mut out, c.recv().await, &big);
    let r = c.recv().await;
    assert!(r.head.starts_with("HTTP/1.1 404"));
    out.push(r);
    check(&mut out, c.recv().await, &mp[1..=3_000_000]);
    check(&mut out, c.recv().await, &big);

    // Several readers at once.
    let addr = server.addr();
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            tokio::spawn(async move {
                let mut c = Conn::open(addr).await;
                let mut got = Vec::new();
                for _ in 0..3 {
                    got.push(c.call("GET", "/bkt/big", &[], b"").await);
                }
                got
            })
        })
        .collect();
    for t in tasks {
        for f in t.await.unwrap() {
            assert!(f.body == big);
        }
    }
    server.shutdown().await;
    out
}

#[tokio::test]
async fn sendfile_and_copy_paths_agree() {
    let before = fakes3::sendfile_bytes();
    let copied = exercise(false).await;
    assert_eq!(fakes3::sendfile_bytes(), before, "sendfile used while disabled");
    let sent = exercise(true).await;
    // Every memfd-sized read above should have gone through sendfile.
    assert!(fakes3::sendfile_bytes() - before > 30 << 20, "sendfile path was not exercised");
    assert_eq!(copied.len(), sent.len());
    for (a, b) in copied.iter().zip(&sent) {
        assert_eq!(a.stable_head(), b.stable_head());
        assert!(a.body == b.body);
    }
}
