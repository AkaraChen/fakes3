//! SelectObjectContent through the real `aws-sdk-s3` client: the supported SQL, CSV and JSON in and
//! out, the event stream, and the errors for what is not supported.

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    CompressionType, CsvInput, CsvOutput, ExpressionType, FileHeaderInfo, InputSerialization, JsonInput, JsonOutput, JsonType,
    OutputSerialization, QuoteFields, RequestProgress, ScanRange, SelectObjectContentEventStream, StorageClass,
};
use fakes3::{Config, Server};

const CSV: &str = "1,alice,10\n2,bob,20\n3,carol,\n4,dave,40\n";
const WITH_HEADER: &str = "id,name,score\n1,alice,10\n2,bob,20\n3,carol,\n4,dave,40\n";
const SSE_C_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const SSE_C_MD5: &str = "hRasmdxgYDKV3nvbahU1MA==";

struct Ctx {
    s3: aws_sdk_s3::Client,
    _server: Server,
}

async fn ctx() -> Ctx {
    let config = Config::default();
    let server = Server::start(config.clone()).await.unwrap();
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(server.endpoint())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(&config.access_key, &config.secret_key, None, None, "test"))
            .force_path_style(true)
            .build(),
    );
    s3.create_bucket().bucket("sel").send().await.unwrap();
    Ctx { s3, _server: server }
}

impl Ctx {
    async fn put(&self, key: &str, body: &str) {
        self.s3
            .put_object()
            .bucket("sel")
            .key(key)
            .body(ByteStream::from(body.as_bytes().to_vec()))
            .send()
            .await
            .unwrap();
    }

    /// Runs `sql` over `key`; `Ok` is the concatenated Records, `Err` the S3 error code.
    async fn run(&self, key: &str, sql: &str, input: InputSerialization, output: OutputSerialization) -> Result<String, String> {
        let out = self
            .s3
            .select_object_content()
            .bucket("sel")
            .key(key)
            .expression(sql)
            .expression_type(ExpressionType::Sql)
            .input_serialization(input)
            .output_serialization(output)
            .send()
            .await
            .map_err(|e| e.code().unwrap_or("?").to_owned())?;
        let mut payload = out.payload;
        let mut text = String::new();
        let mut end = false;
        while let Some(ev) = payload.recv().await.map_err(|e| e.to_string())? {
            match ev {
                SelectObjectContentEventStream::Records(r) => text.push_str(&String::from_utf8(r.payload.unwrap().into_inner()).unwrap()),
                SelectObjectContentEventStream::End(_) => end = true,
                _ => {}
            }
        }
        assert!(end, "no End event");
        Ok(text)
    }

    async fn csv(&self, sql: &str) -> Result<String, String> {
        self.run("data.csv", sql, csv_in(FileHeaderInfo::None), csv_out()).await
    }

    async fn csv_header(&self, sql: &str, h: FileHeaderInfo) -> Result<String, String> {
        self.run("header.csv", sql, csv_in(h), csv_out()).await
    }

    async fn json(&self, key: &str, sql: &str, ty: JsonType) -> Result<String, String> {
        self.run(key, sql, json_in(ty), json_out()).await
    }
}

fn csv_in(h: FileHeaderInfo) -> InputSerialization {
    InputSerialization::builder()
        .csv(CsvInput::builder().file_header_info(h).build())
        .compression_type(CompressionType::None)
        .build()
}

fn csv_out() -> OutputSerialization {
    OutputSerialization::builder().csv(CsvOutput::builder().build()).build()
}

fn json_in(ty: JsonType) -> InputSerialization {
    InputSerialization::builder().json(JsonInput::builder().r#type(ty).build()).build()
}

fn json_out() -> OutputSerialization {
    OutputSerialization::builder().json(JsonOutput::builder().build()).build()
}

#[tokio::test]
async fn csv_projection_filter_and_limit() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    assert_eq!(c.csv("select * from s3object").await.unwrap(), CSV);
    assert_eq!(
        c.csv("SELECT _2, _1 FROM S3Object s WHERE s._1 > 1 and s._1 < 4").await.unwrap(),
        "bob,2\ncarol,3\n"
    );
    assert_eq!(
        c.csv("select _2 from s3object where _2 like '%a%' or _1 = 2 limit 2")
            .await
            .unwrap(),
        "alice\nbob\n"
    );
    assert_eq!(
        c.csv("select _1 from s3object where not _1 in (1, 2) and _2 not like 'x%'")
            .await
            .unwrap(),
        "3\n4\n"
    );
    assert_eq!(c.csv("select _1 from s3object where _1 between 2 and 3").await.unwrap(), "2\n3\n");
    assert_eq!(
        c.csv("select _1 * 10 + 1, _1 / 2, _1 % 2, -_1 from s3object where _1 = 3")
            .await
            .unwrap(),
        "31,1,1,-3\n"
    );
    assert_eq!(c.csv("select _1 from s3object where _3 is null").await.unwrap(), "");
    assert_eq!(c.csv("select _1 from s3object where _3 = ''").await.unwrap(), "3\n");
    assert_eq!(c.csv("select _1 from s3object where _9 is null limit 1").await.unwrap(), "1\n");
    assert_eq!(
        c.csv("select 1 + 2.5, 'a' || 'b', upper(_2), lower('XY'), char_length(_2) from s3object limit 1")
            .await
            .unwrap(),
        "3.5,ab,ALICE,xy,5\n"
    );
}

#[tokio::test]
async fn functions_and_cast() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    let q = |sql: &'static str| c.csv(sql);
    assert_eq!(q("select cast(_3 as int) + 1 from s3object where _1 = 1").await.unwrap(), "11\n");
    assert_eq!(q("select int(_3) from s3object where _1 = 2").await.unwrap(), "20\n");
    assert_eq!(
        q("select cast(_1 as decimal(10,2)) / 4 from s3object where _1 = 1").await.unwrap(),
        "0.25\n"
    );
    assert_eq!(
        q("select substring(_2, 2, 3), substring(_2 from 4), trim(both 'a' from _2) from s3object where _1 = 1")
            .await
            .unwrap(),
        "lic,ce,lice\n"
    );
    assert_eq!(
        q("select trim('  x '), trim(leading from '  x '), trim(trailing from '  x ') from s3object limit 1")
            .await
            .unwrap(),
        "x,x ,  x\n"
    );
    assert_eq!(
        q("select coalesce(nullif(_3, ''), 'none'), nullif(_1, 1) from s3object where _1 < 4 and _1 > 0")
            .await
            .unwrap(),
        "10,\n20,2\nnone,3\n"
    );
    assert_eq!(
        q("select case when _1 = 1 then 'one' when _1 = 2 then 'two' else 'many' end, case _1 when 3 then 'x' end from s3object")
            .await
            .unwrap(),
        "one,\ntwo,\nmany,x\nmany,\n"
    );
    assert_eq!(
        q("select _2 from s3object where _2 like 'c_r%l' escape '|'").await.unwrap(),
        "carol\n"
    );
    // a double-quoted name that is no column is a string, as in Ceph's s3select
    assert_eq!(q("select lower(\"AB12cd$$\") from s3object limit 1").await.unwrap(), "ab12cd$$\n");
    assert_eq!(q("select _1 from s3object where _2 = \"bob\"").await.unwrap(), "2\n");
    assert_eq!(
        q("select _1 from s3object where _1 like \"_\" and \"_3\" = '_3'").await.unwrap(),
        "1\n2\n3\n4\n"
    );
}

#[tokio::test]
async fn aggregates() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    assert_eq!(
        c.csv("select count(*), count(_3), count(0) from s3object").await.unwrap(),
        "4,4,4\n"
    );
    assert_eq!(
        c.csv("select sum(cast(_3 as int)), min(cast(_3 as int)), max(cast(_3 as int)) from s3object where _3 <> ''")
            .await
            .unwrap(),
        "70,10,40\n"
    );
    assert_eq!(
        c.csv("select avg(cast(_1 as int)), sum(_1) + 1 from s3object").await.unwrap(),
        "2.5,11\n"
    );
    assert_eq!(c.csv("select max(_2), min(_2) from s3object").await.unwrap(), "dave,alice\n");
    assert_eq!(
        c.csv("select count(*), sum(_1) from s3object where _1 > 100").await.unwrap(),
        "0,\n"
    );
}

#[tokio::test]
async fn aliases_and_headers() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    c.put("header.csv", WITH_HEADER).await;
    let used = |sql: &'static str| c.csv_header(sql, FileHeaderInfo::Use);
    assert_eq!(
        used("select name, s.score from s3object s where id > 2").await.unwrap(),
        "carol,\ndave,40\n"
    );
    assert_eq!(used("select NAME from s3object where \"id\" = 1").await.unwrap(), "alice\n");
    assert_eq!(used("select * from s3object limit 1").await.unwrap(), "1,alice,10\n");
    assert_eq!(used("select _2 from s3object where _1 = 4").await.unwrap(), "dave\n");
    assert_eq!(used("select nope from s3object").await.unwrap_err(), "MissingHeaders");
    let ignored = c
        .csv_header("select _2 from s3object limit 1", FileHeaderInfo::Ignore)
        .await
        .unwrap();
    assert_eq!(ignored, "alice\n");
    assert_eq!(
        c.csv("select nope from s3object").await.unwrap_err(),
        "EvaluatorBindingDoesNotExist"
    );
    // select aliases are visible to later projections and to WHERE
    assert_eq!(
        c.csv("select _1 as a, a * 2 as b from s3object where b > 4 and a < 4")
            .await
            .unwrap(),
        "3,6\n"
    );
    let cyclic = c.csv("select a + 1 as b, b + 1 as a from s3object").await.unwrap_err();
    assert_eq!(cyclic, "ParseUnsupportedAlias");
}

#[tokio::test]
async fn csv_dialects_and_output() {
    let c = ctx().await;
    c.put("pipes", "1|\"x|y\"|z\t2||\"q\"\"r\"\t").await;
    let input = InputSerialization::builder()
        .csv(CsvInput::builder().field_delimiter("|").record_delimiter("\t").build())
        .build();
    let out = c
        .run("pipes", "select _2, _3 from s3object", input.clone(), csv_out())
        .await
        .unwrap();
    assert_eq!(out, "x|y,z\n,\"q\"\"r\"\n");
    let always = OutputSerialization::builder()
        .csv(
            CsvOutput::builder()
                .quote_fields(QuoteFields::Always)
                .field_delimiter(";")
                .record_delimiter("#")
                .build(),
        )
        .build();
    let out = c.run("pipes", "select _1, _3 from s3object", input, always).await.unwrap();
    assert_eq!(out, "\"1\";\"z\"#\"2\";\"q\"\"r\"#");
}

#[tokio::test]
async fn json_lines_document_and_output() {
    let c = ctx().await;
    c.put(
        "lines",
        "{\"a\":1,\"b\":{\"c\":\"x\"}}\n{\"a\":2,\"b\":{\"c\":\"y\"},\"d\":[7,8]}\n{\"a\":3}\n",
    )
    .await;
    c.put("doc", "{\"root\": [{\"n\": 1}, {\"n\": 2}, {\"n\": 3}], \"name\": \"r\"}")
        .await;
    let lines = |sql: &'static str| c.json("lines", sql, JsonType::Lines);
    assert_eq!(
        lines("select * from s3object").await.unwrap(),
        "{\"a\":1,\"b\":{\"c\":\"x\"}}\n{\"a\":2,\"b\":{\"c\":\"y\"},\"d\":[7,8]}\n{\"a\":3}\n"
    );
    assert_eq!(
        lines("select s.b.c, s.d[1] as second from s3object s where s.a >= 2")
            .await
            .unwrap(),
        "{\"c\":\"y\",\"second\":8}\n{}\n"
    );
    assert_eq!(
        lines("select count(*), sum(s.a), max(s.a) from s3object s").await.unwrap(),
        "{\"_1\":3,\"_2\":6,\"_3\":3}\n"
    );
    assert_eq!(lines("select a from s3object where b.c is missing").await.unwrap(), "{\"a\":3}\n");
    assert_eq!(
        c.json("doc", "select count(*) from s3object[*]", JsonType::Document).await.unwrap(),
        "{\"_1\":1}\n"
    );
    assert_eq!(
        c.json("doc", "select count(*) from s3object[*].root", JsonType::Document)
            .await
            .unwrap(),
        "{\"_1\":3}\n"
    );
    assert_eq!(
        c.json("doc", "select r.n from s3object[*].root r where r.n > 1", JsonType::Document)
            .await
            .unwrap(),
        "{\"n\":2}\n{\"n\":3}\n"
    );
    assert_eq!(
        c.json("doc", "select _1.n from s3object[*].root", JsonType::Document)
            .await
            .unwrap(),
        "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n"
    );
    // JSON in, CSV out
    let out = c
        .run(
            "lines",
            "select s.a, s.b.c from s3object s where s.a < 3",
            json_in(JsonType::Lines),
            csv_out(),
        )
        .await
        .unwrap();
    assert_eq!(out, "1,x\n2,y\n");
    assert_eq!(
        c.json("lines", "select * from s3object[", JsonType::Lines).await.unwrap_err(),
        "ParseUnexpectedToken"
    );
    c.put("bad", "{\"a\":").await;
    assert_eq!(
        c.json("bad", "select * from s3object", JsonType::Lines).await.unwrap_err(),
        "JSONParsingError"
    );
}

#[tokio::test]
async fn stats_progress_and_scan_range() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    let out =
        c.s3.select_object_content()
            .bucket("sel")
            .key("data.csv")
            .expression("select _2 from s3object")
            .expression_type(ExpressionType::Sql)
            .input_serialization(csv_in(FileHeaderInfo::None))
            .output_serialization(csv_out())
            .request_progress(RequestProgress::builder().enabled(true).build())
            .send()
            .await
            .unwrap();
    let mut payload = out.payload;
    let (mut kinds, mut returned) = (Vec::new(), 0);
    while let Some(ev) = payload.recv().await.unwrap() {
        match ev {
            SelectObjectContentEventStream::Records(r) => {
                returned += r.payload.unwrap().as_ref().len() as i64;
                kinds.push("records");
            }
            SelectObjectContentEventStream::Progress(p) => {
                let d = p.details.unwrap();
                assert_eq!(
                    (d.bytes_scanned(), d.bytes_processed(), d.bytes_returned()),
                    (Some(CSV.len() as i64), Some(CSV.len() as i64), Some(returned))
                );
                kinds.push("progress");
            }
            SelectObjectContentEventStream::Stats(s) => {
                let d = s.details.unwrap();
                assert_eq!((d.bytes_scanned(), d.bytes_returned()), (Some(CSV.len() as i64), Some(returned)));
                kinds.push("stats");
            }
            SelectObjectContentEventStream::End(_) => kinds.push("end"),
            _ => {}
        }
    }
    assert_eq!(kinds, ["records", "progress", "stats", "end"]);
    assert_eq!(returned, "alice\nbob\ncarol\ndave\n".len() as i64);

    // records that start inside the range: "2,bob,20" starts at byte 11, "3,carol," at 20
    let range = ScanRange::builder().start(11).end(20).build();
    let out =
        c.s3.select_object_content()
            .bucket("sel")
            .key("data.csv")
            .expression("select _1 from s3object")
            .expression_type(ExpressionType::Sql)
            .input_serialization(csv_in(FileHeaderInfo::None))
            .output_serialization(csv_out())
            .scan_range(range)
            .send()
            .await
            .unwrap();
    let mut payload = out.payload;
    let mut text = String::new();
    while let Some(ev) = payload.recv().await.unwrap() {
        if let SelectObjectContentEventStream::Records(r) = ev {
            text.push_str(std::str::from_utf8(r.payload.unwrap().as_ref()).unwrap());
        }
    }
    assert_eq!(text, "2\n3\n");
}

#[tokio::test]
async fn errors() {
    let c = ctx().await;
    c.put("data.csv", CSV).await;
    for (sql, code) in [
        ("select * from s3object group by _1", "UnsupportedSqlOperation"),
        ("select * from s3object order by _1", "UnsupportedSqlOperation"),
        ("select extract(year from _1) from s3object", "UnsupportedSqlOperation"),
        ("select frobnicate(_1) from s3object", "UnsupportedFunction"),
        ("select from s3object", "ParseUnexpectedToken"),
        ("select _1", "ParseSelectMissingFrom"),
        ("select * from s3object where _1 = ", "ParseUnexpectedToken"),
        ("select * from elsewhere", "ParseUnexpectedToken"),
        ("select * from s3object where _1 ^ 2", "LexerInvalidChar"),
        ("select _0 from s3object", "InvalidColumnIndex"),
        ("select cast(_2 as int) from s3object", "CastFailed"),
        ("select _1 / 0 from s3object", "EvaluatorInvalidArguments"),
        ("select _1, count(*) from s3object", "UnsupportedSqlOperation"),
    ] {
        assert_eq!(c.csv(sql).await.unwrap_err(), code, "{sql}");
    }
    // unknown column index beyond the row is NULL, not an error
    assert_eq!(c.csv("select _99 from s3object limit 1").await.unwrap(), "\n");

    let gz = InputSerialization::builder()
        .csv(CsvInput::builder().build())
        .compression_type(CompressionType::Gzip)
        .build();
    assert_eq!(
        c.run("data.csv", "select * from s3object", gz, csv_out()).await.unwrap_err(),
        "NotImplemented"
    );
    let both = InputSerialization::builder()
        .csv(CsvInput::builder().build())
        .json(JsonInput::builder().build())
        .build();
    assert_eq!(
        c.run("data.csv", "select * from s3object", both, csv_out()).await.unwrap_err(),
        "ObjectSerializationConflict"
    );
    let none = InputSerialization::builder().build();
    assert_eq!(
        c.run("data.csv", "select * from s3object", none, csv_out()).await.unwrap_err(),
        "MissingRequiredParameter"
    );
    let bad_quote = OutputSerialization::builder()
        .csv(CsvOutput::builder().field_delimiter(",,").build())
        .build();
    assert_eq!(
        c.run("data.csv", "select * from s3object", csv_in(FileHeaderInfo::None), bad_quote)
            .await
            .unwrap_err(),
        "InvalidRequestParameter"
    );
    assert_eq!(
        c.run("missing", "select * from s3object", csv_in(FileHeaderInfo::None), csv_out())
            .await
            .unwrap_err(),
        "NoSuchKey"
    );
}

#[tokio::test]
async fn archived_and_sse_c_objects() {
    let c = ctx().await;
    c.s3.put_object()
        .bucket("sel")
        .key("cold")
        .storage_class(StorageClass::Glacier)
        .body(ByteStream::from_static(b"1,2\n"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        c.run("cold", "select * from s3object", csv_in(FileHeaderInfo::None), csv_out())
            .await
            .unwrap_err(),
        "InvalidObjectState"
    );

    c.s3.put_object()
        .bucket("sel")
        .key("secret")
        .sse_customer_algorithm("AES256")
        .sse_customer_key(SSE_C_KEY)
        .sse_customer_key_md5(SSE_C_MD5)
        .body(ByteStream::from_static(b"1,2\n"))
        .send()
        .await
        .unwrap();
    let q = |key: Option<&str>| {
        let mut r =
            c.s3.select_object_content()
                .bucket("sel")
                .key("secret")
                .expression("select _2 from s3object")
                .expression_type(ExpressionType::Sql)
                .input_serialization(csv_in(FileHeaderInfo::None))
                .output_serialization(csv_out());
        if let Some(k) = key {
            r = r
                .sse_customer_algorithm("AES256")
                .sse_customer_key(k)
                .sse_customer_key_md5(if k == SSE_C_KEY { SSE_C_MD5 } else { "dT1y7SEiqn6YCJ3QeqwoIw==" });
        }
        r.send()
    };
    assert!(q(None).await.is_err(), "reading an SSE-C object without its key must fail");
    assert!(
        q(Some("ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=")).await.is_err(),
        "and with another key"
    );
    let mut ok = q(Some(SSE_C_KEY)).await.unwrap().payload;
    let mut text = String::new();
    while let Some(ev) = ok.recv().await.unwrap() {
        if let SelectObjectContentEventStream::Records(r) = ev {
            text.push_str(std::str::from_utf8(r.payload.unwrap().as_ref()).unwrap());
        }
    }
    assert_eq!(text, "2\n");
}

/// The spellings Ceph's s3select (and so the ceph/s3-tests) use: `stdin` as the table, `int()`,
/// `count(0)`, double-quoted string literals and `[a-z]` classes in LIKE.
#[tokio::test]
async fn ceph_dialect() {
    let c = ctx().await;
    let (mut x, mut rows) = (12345u64, String::new());
    let mut next = move || {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (x >> 33) % 6
    };
    for _ in 0..3000 {
        let tail = ["vwxyzzvwxyz", "abcdeaeiou", "cbcdQvwxyz", "zzzzzzyz"][(next() % 4) as usize];
        rows.push_str(&format!("{},{},cbcd{tail},{tail},\n", next(), next()));
    }
    c.put("data.csv", &rows).await;
    let same = |a: &'static str, b: &'static str| {
        let c = &c;
        async move { assert_eq!(c.csv(a).await.unwrap(), c.csv(b).await.unwrap(), "{a} vs {b}") }
    };
    same(
        "select count(0) from stdin where nullif(_1,_2) is null",
        "select count(0) from stdin where _1 = _2",
    )
    .await;
    same(
        "select count(0) from stdin where not nullif(_1,_2) is null",
        "select count(0) from stdin where _1 != _2",
    )
    .await;
    same(
        "select count(*) from s3object where (nullif(_1,_2) = _1 = true)",
        "select count(*) from s3object where _1 != _2",
    )
    .await;
    same(
        "select int(_1) from stdin where int(_1) in(1,0)",
        "select int(_1) from stdin where int(_1) = 1 or int(_1) = 0",
    )
    .await;
    same(
        "select int(_2) from stdin where int(_2)*2 in(int(_1)*2,int(_2)*3)",
        "select int(_2) from stdin where int(_2)*2 = int(_1)*2 or int(_2)*2 = int(_2)*3",
    )
    .await;
    same(
        "select int(_1) from stdin where character_length(_1) = 1 and substring(_1,1,1) in (\"3\")",
        "select int(_1) from stdin where _1 like \"3\"",
    )
    .await;
    same(
        "select count(*) from stdin where _3 like \"%y[y-z]\"",
        "select count(*) from stdin where substring(_3,character_length(_3),1) between \"y\" and \"z\" and substring(_3,character_length(_3)-1,1) = \"y\"",
    )
    .await;
    same(
        "select count(*) from s3object where _4 like \"%aeio%\"",
        "select count(*) from s3object where substring(_4,6,4) = \"aeio\"",
    )
    .await;
    same(
        "select count(*) from s3object where trim(_4) = \"abcdeaeiou\"",
        "select count(*) from s3object where _4 = \"abcdeaeiou\"",
    )
    .await;
    assert_eq!(
        c.csv("select count(0) from s3object where 4.0*(12.0-2.0) > 30.0 and 7.0/2.0 = 3.5")
            .await
            .unwrap(),
        "3000\n"
    );
    assert_eq!(c.csv("select (3.0*(4.0+1.0))/2.0 from s3object limit 1").await.unwrap(), "7.5\n");
    assert_eq!(c.csv("select lower(\"AB12cd$$\") from stdin limit 1").await.unwrap(), "ab12cd$$\n");
    // aliases in WHERE and in later projections, with the same result as the spelled-out query
    same(
        "select int(_1) as a1, int(_2) as a2, (a1+a2) as a3 from s3object where a3>3 and a3<7",
        "select int(_1),int(_2),int(_1)+int(_2) from s3object where (int(_1)+int(_2))>3 and (int(_1)+int(_2))<7",
    )
    .await;
    let cyclic = c
        .csv("select int(_1) as a1,int(_2) as a2, a1+a4 as a3, a5+a1 as a4, int(_1)+a3 as a5 from s3object")
        .await;
    assert_eq!(cyclic.unwrap_err(), "ParseUnsupportedAlias");
}
