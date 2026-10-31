# S3 Select

fakes3 实现了 `SelectObjectContent` 的一个实用子集：CSV 和 JSON 输入，一小部分 SQL，CSV / JSON 输出，
以及 Records、Progress、Stats、End 事件流。代码在 `src/select/`，没有 Select 请求时零开销。
AWS 已不对新客户开放 S3 Select，所以这里只做到“测试里常见的查询能跑”，不追求完整的 PartiQL。

```bash
aws s3api select-object-content --endpoint-url http://127.0.0.1:9000 --bucket b --key data.csv \
  --expression "select s.name, cast(s.score as int) * 2 from s3object s where s.score > 10 limit 5" \
  --expression-type SQL \
  --input-serialization '{"CSV": {"FileHeaderInfo": "USE"}}' \
  --output-serialization '{"CSV": {}}' /dev/stdout
```

## 请求

| 选项 | 支持 |
|---|---|
| `ExpressionType` | 只有 `SQL`，其他返回 `InvalidExpressionType` |
| 输入 | `CSV`、`JSON`（`LINES` 和 `DOCUMENT`，实际上都按“一串 JSON 值”读，所以换行排版随意）。`Parquet` 返回 501 `NotImplemented` |
| `CompressionType` | 只有 `NONE`。`GZIP`、`BZIP2` 返回 501 `NotImplemented`，其他值 `InvalidCompressionFormat` |
| CSV 输入 | `FileHeaderInfo`（`NONE` / `IGNORE` / `USE`）、`FieldDelimiter`、`RecordDelimiter`（1~2 个字符，`\r\n` 可以）、`QuoteCharacter`、`QuoteEscapeCharacter`、`Comments`；分隔符和引号必须是单个 ASCII 字符，否则 `InvalidRequestParameter`。引号里可以有分隔符和换行（`AllowQuotedRecordDelimiter` 总是开着）；引号在字段中间时就是普通字符 |
| 输出 | `CSV`（`FieldDelimiter`、`RecordDelimiter`、`QuoteCharacter`、`QuoteEscapeCharacter`、`QuoteFields` = `ALWAYS` / `ASNEEDED`）或 `JSON`（`RecordDelimiter`）。两个都给或都不给分别是 `ObjectSerializationConflict`、`MissingRequiredParameter` |
| `RequestProgress` | `Enabled=true` 时在 Stats 前发一个 Progress 事件。`BytesScanned` = `BytesProcessed` = 扫描的字节数，`BytesReturned` = Records 负载的总长 |
| `ScanRange` | CSV 和 JSON `LINES`：起点落在 `[Start, End]` 里的记录会被处理（只给 `End` 表示对象最后 `End` 个字节）。`DOCUMENT` 返回 `InvalidScanRange` |
| SSE-C | 和 GET 一样，必须带同一个 key |
| 归档对象 | `GLACIER` / `DEEP_ARCHIVE` 等没恢复时返回 403 `InvalidObjectState` |

整个查询在发出第一个字节之前就跑完了，所以查询出错（包括执行期的 `CastFailed`）是一个普通的 HTTP 400，
而不是事件流中间的错误事件。结果全部缓存在内存里再发送，别拿它去 `SELECT *` 一个几 GB 的对象。

## SQL

`SELECT <投影> FROM S3Object[路径] [别名] [WHERE <表达式>] [LIMIT n]`，关键字不分大小写，末尾的 `;` 可有可无。
`FROM stdin` 和 `FROM S3Object` 等价（Ceph 的写法）。

- **投影**：`*`、`别名.*`、逗号分隔的表达式，每个可以带 `AS 名字`。
  前面的投影别名可以在后面的投影和 `WHERE` 里使用；别名互相引用成环时返回 `ParseUnsupportedAlias`。
- **列**：CSV 用 `_1`、`_2`…（`_0` 或超过 1048576 是 `InvalidColumnIndex`，超过本行列数是 NULL），
  `FileHeaderInfo=USE` 时还可以用表头名（先精确匹配，再不分大小写；表头里没有的名字是 `MissingHeaders`）。
  JSON 用 `别名.a.b[0]["c"]` 或直接写字段名；不存在的字段是 MISSING（JSON 输出里省略，`IS MISSING` 为真）。
  没有显式别名时 JSON 记录的隐含别名是 `_1`（`_1.c1` 等于 `s.c1`）。
- **FROM 路径**（JSON）：`S3Object[*]`、`S3Object[*].root`、`S3Object.a.b`、`[n]`。有路径且指向数组时，数组的每个元素是一条记录；
  没有路径时每个顶层 JSON 值是一条记录。
- **运算符**：`+ - * / %`（整数除法截断，除以 0 是 `EvaluatorInvalidArguments`）、`||`、
  `= != <> < <= > >=`、`AND OR NOT`（三值逻辑）、`IS [NOT] NULL`、`IS [NOT] MISSING`、
  `[NOT] LIKE ... [ESCAPE c]`（`%`、`_`，另外支持 Ceph 的 `[a-z]` 字符类）、`[NOT] IN (...)`、`[NOT] BETWEEN a AND b`、`CASE`。
- **函数**：`CAST(x AS INT|FLOAT|DECIMAL|NUMERIC|STRING|VARCHAR|CHAR|BOOL)`（失败是 `CastFailed`）、
  `LOWER`、`UPPER`、`CHAR_LENGTH` / `CHARACTER_LENGTH`、`SUBSTRING(s, from[, len])` 和 `SUBSTRING(s FROM n [FOR m])`、
  `TRIM([BOTH|LEADING|TRAILING] [chars] FROM s)` 和 `TRIM(s)`、`COALESCE`、`NULLIF`；
  聚合 `COUNT(*)`、`COUNT(x)`、`SUM`、`MIN`、`MAX`、`AVG`（投影里有聚合时只返回一行，聚合外不能再有列）。
- **类型**：CSV 的值都是字符串，和数字比较或做算术时按数字解析（解析不了在算术里是 `IncorrectSqlFunctionArgumentType`，在比较里是 NULL）。
  CSV 里的空字段是空字符串，不是 NULL。浮点输出整数值时带 `.0`（`3.0`）。

### 为了跑 ceph/s3-tests 加的、AWS 没有的写法

- `INT(x)` 等于 `CAST(x AS INT)`。
- 双引号括起来的名字先当表头名 / JSON 字段 / 投影别名找（`"_3"` 这样的位置名不算），找不到就当字符串字面量（`lower("AB12cd$$")`、`_1 like "_3"`）；AWS 里它只是标识符。
- LIKE 里的 `[a-z]`、`FROM stdin`、`COUNT(0)`（当然也能用）。

## 不支持的东西和错误码

| 查询 | 结果 |
|---|---|
| `GROUP BY`、`ORDER BY`、`HAVING`、`JOIN`、`UNION`、`DISTINCT`、`OFFSET` | 400 `UnsupportedSqlOperation` |
| 日期时间函数（`TO_TIMESTAMP`、`EXTRACT`、`DATE_ADD`、`DATE_DIFF`、`UTCNOW`、`TO_STRING`）、`CAST(.. AS TIMESTAMP)` | 400 `UnsupportedSqlOperation` |
| 其他不认识的函数 | 400 `UnsupportedFunction` |
| 聚合里套聚合、`WHERE` 里的聚合、聚合和裸列混用 | 400 `UnsupportedSqlOperation` |
| 语法错误 | 400 `ParseUnexpectedToken`（缺 `FROM` 是 `ParseSelectMissingFrom`，缺关键字是 `ParseExpectedKeyword`，非法字符是 `LexerInvalidChar`，`LIMIT -1` 是 `NegativeLimit`，类型名不对是 `ParseExpectedTypeName`） |
| 对象不是合法 UTF-8 / JSON | 400 `InvalidTextEncoding` / `JSONParsingError` |
| 没有表头却用了列名 | 400 `EvaluatorBindingDoesNotExist` |

## 和 AWS 的已知差别

- 运行期错误是 HTTP 400，不是事件流里的错误事件（见上）。
- 数值用 `i64` / `f64` 计算，AWS 是十进制数；`SELECT 0.1 + 0.2` 会得到 `0.30000000000000004`。
- CSV 输出的 `ASNEEDED` 在字段含分隔符、引号、换行时加引号，其他情况不加；NULL 和 MISSING 输出为空。
- 引号在字段中间（`a="b,c"`）按普通字符处理，字段会在逗号处断开；AWS 对这种输入的行为没有文档。
- 没有 Continuation（保活）事件。

测试在 `tests/select.rs`（用真实的 aws-sdk-s3）和 `src/select/` 里的单元测试。
