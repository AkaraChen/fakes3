//! Direct XML rendering of ListObjects, ListObjectsV2 and ListObjectVersions responses.
//!
//! s3s serializes a response by walking a DTO, so every listed key first costs an `Object` (a
//! cloned key, a cloned ETag, an `Owner` with two strings) and then a trip through the generic
//! serializer. The handlers below skip both: they walk the index and write the bytes s3s would
//! have written straight into one buffer, sized from the page, and hand s3s an empty output. The
//! rendered body rides back on the response as [`PreRendered`] and `lib.rs` swaps it in once s3s
//! is done, so everything s3s does around the handler (signature checks, access hooks, error
//! handling) and everything fakes3 does around s3s (CORS, request ids, the request log) is
//! untouched.
//!
//! The output must be byte-for-byte what s3s writes. The element order and the escaping follow
//! s3s' generated serializers and quick-xml's `escape`; `tests/list_xml.rs` compares both paths on
//! randomized stores. Whatever this module cannot render identically makes the handler fall back
//! to the DTO path: a string with a control character (s3s turns those into a 500) and a
//! timestamp outside 1970..=9999.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use s3s::dto::*;

use crate::archive;
use crate::objext;
use crate::s3impl::{self, FakeS3, ListEnd, VersionsEnd};
use crate::store::{Bucket, Index, Object as StoreObject, Version};

/// A response body rendered by this module; s3s' own body for the same response is empty.
#[derive(Clone)]
pub(crate) struct PreRendered(pub Bytes);

static FAST: AtomicU64 = AtomicU64::new(0);
static FALLBACK: AtomicU64 = AtomicU64::new(0);

/// How many list responses were rendered directly and how many fell back to the DTO path since
/// the process started. For tests that must know the fast path was really exercised.
pub fn counters() -> (u64, u64) {
    (FAST.load(Ordering::Relaxed), FALLBACK.load(Ordering::Relaxed))
}

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const OPEN: &str = "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">";
const OPEN_VERSIONS: &str = "<ListVersionsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">";

/// What one byte of text content needs, per quick-xml's `escape`: nothing, an entity, or (a
/// control character) a response s3s refuses to send.
const PLAIN: u8 = 0;
const BAD: u8 = 1;
const ENTITIES: [&str; 6] = ["&lt;", "&gt;", "&amp;", "&apos;", "&quot;", "&#13;"];

const fn classify() -> [u8; 256] {
    let mut t = [PLAIN; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = BAD;
        i += 1;
    }
    t[b'\t' as usize] = PLAIN;
    t[b'\n' as usize] = PLAIN;
    t[b'<' as usize] = 2;
    t[b'>' as usize] = 3;
    t[b'&' as usize] = 4;
    t[b'\'' as usize] = 5;
    t[b'"' as usize] = 6;
    t[b'\r' as usize] = 7;
    t
}

static CLASS: [u8; 256] = classify();

/// A growing XML fragment. `bad` is set when something went in that s3s would not have sent.
pub(crate) struct Xml {
    buf: Vec<u8>,
    bad: bool,
}

impl Xml {
    fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            bad: false,
        }
    }

    fn raw(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    /// Text content with the characters quick-xml escapes; a control character other than tab,
    /// line feed and carriage return marks the fragment bad.
    fn text(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let mut from = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let class = CLASS[b as usize];
            if class == PLAIN {
                continue;
            }
            self.buf.extend_from_slice(&bytes[from..i]);
            from = i + 1;
            if class == BAD {
                self.bad = true;
            } else {
                self.raw(ENTITIES[class as usize - 2]);
            }
        }
        self.buf.extend_from_slice(&bytes[from..]);
    }

    /// `encoding-type=url` text: what `s3impl::url_encode` returns, without building it. Only
    /// `A-Za-z0-9*-._/` survive, so nothing needs XML escaping afterwards.
    fn url(&mut self, s: &str) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        self.buf.reserve(s.len());
        for &b in s.as_bytes() {
            match b {
                b'*' | b'-' | b'.' | b'_' | b'/' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => self.buf.push(b),
                b' ' => self.buf.extend_from_slice(b"%20"),
                _ => self
                    .buf
                    .extend_from_slice(&[b'%', HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]]),
            }
        }
    }

    fn text_or_url(&mut self, s: &str, url: bool) {
        if url {
            self.url(s);
        } else {
            self.text(s);
        }
    }

    /// `<name>text</name>`.
    fn el(&mut self, name: &str, s: &str) {
        self.raw("<");
        self.raw(name);
        self.raw(">");
        self.text(s);
        self.raw("</");
        self.raw(name);
        self.raw(">");
    }

    /// Like [`Xml::el`], percent-encoded when the request asked for `encoding-type=url`.
    fn el_enc(&mut self, name: &str, s: &str, url: bool) {
        self.raw("<");
        self.raw(name);
        self.raw(">");
        self.text_or_url(s, url);
        self.raw("</");
        self.raw(name);
        self.raw(">");
    }

    fn el_int(&mut self, name: &str, n: i64) {
        self.raw("<");
        self.raw(name);
        self.raw(">");
        self.int(n);
        self.raw("</");
        self.raw(name);
        self.raw(">");
    }

    fn el_bool(&mut self, name: &str, b: bool) {
        self.raw("<");
        self.raw(name);
        self.raw(">");
        self.raw(if b { "true" } else { "false" });
        self.raw("</");
        self.raw(name);
        self.raw(">");
    }

    fn int(&mut self, n: i64) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        let mut rest = n.unsigned_abs();
        loop {
            at -= 1;
            digits[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        if n < 0 {
            self.buf.push(b'-');
        }
        self.buf.extend_from_slice(&digits[at..]);
    }

    /// `2006-01-02T15:04:05.000Z`, the millisecond-truncated form s3s writes. Years outside
    /// 1970..=9999 are left to s3s.
    fn timestamp(&mut self, t: SystemTime) {
        let Some(d) = t.duration_since(UNIX_EPOCH).ok().filter(|d| d.as_secs() < 253_402_300_800) else {
            self.bad = true;
            return;
        };
        let secs = d.as_secs();
        let (y, mo, day) = objext::civil_from_days((secs / 86_400) as i64);
        let rem = secs % 86_400;
        let mut b = *b"0000-00-00T00:00:00.000Z";
        let mut digits = |at: std::ops::Range<usize>, mut n: u64| {
            for d in b[at].iter_mut().rev() {
                *d = b'0' + (n % 10) as u8;
                n /= 10;
            }
        };
        digits(0..4, y as u64);
        digits(5..7, u64::from(mo));
        digits(8..10, u64::from(day));
        digits(11..13, rem / 3600);
        digits(14..16, rem % 3600 / 60);
        digits(17..19, rem % 60);
        digits(20..23, u64::from(d.subsec_millis()));
        self.buf.extend_from_slice(&b);
    }

    fn el_timestamp(&mut self, name: &str, t: SystemTime) {
        self.raw("<");
        self.raw(name);
        self.raw(">");
        self.timestamp(t);
        self.raw("</");
        self.raw(name);
        self.raw(">");
    }

    /// The ETag as s3s writes it: quoted, but not escaped.
    fn etag(&mut self, e: &str) {
        self.raw("<ETag>\"");
        if e.bytes().any(|b| CLASS[b as usize] == BAD) {
            self.bad = true;
        }
        self.raw(e);
        self.raw("\"</ETag>");
    }

    fn common_prefix(&mut self, p: &str, url: bool) {
        self.raw("<CommonPrefixes><Prefix>");
        self.text_or_url(p, url);
        self.raw("</Prefix></CommonPrefixes>");
    }

    fn storage_class(&mut self, o: &StoreObject) {
        let class = o
            .ext
            .as_deref()
            .and_then(|e| e.storage_class.as_ref())
            .map_or(ObjectStorageClass::STANDARD, |c| c.as_str());
        self.raw("<StorageClass>");
        self.text(class);
        self.raw("</StorageClass>");
    }

    /// A `<Contents>` entry of ListObjects / ListObjectsV2.
    fn contents(&mut self, key: &str, o: &StoreObject, url: bool, owner: Option<&str>, restore: Option<archive::Archived>) {
        self.raw("<Contents>");
        self.etag(&o.e_tag);
        self.el_enc("Key", key, url);
        self.el_timestamp("LastModified", o.last_modified);
        if let Some(owner) = owner {
            self.raw(owner);
        }
        if let Some(status) = restore.and_then(|a| a.restore_status()) {
            self.restore_status(&status);
        }
        self.el_int("Size", o.data.size as i64);
        self.storage_class(o);
        self.raw("</Contents>");
    }

    fn restore_status(&mut self, s: &RestoreStatus) {
        self.raw("<RestoreStatus>");
        if let Some(b) = s.is_restore_in_progress {
            self.el_bool("IsRestoreInProgress", b);
        }
        if let Some(t) = &s.restore_expiry_date {
            self.raw("<RestoreExpiryDate>");
            // Rare enough to leave to s3s' own formatter.
            if t.format(TimestampFormat::DateTime, &mut self.buf).is_err() {
                self.bad = true;
            }
            self.raw("</RestoreExpiryDate>");
        }
        self.raw("</RestoreStatus>");
    }

    fn version(&mut self, key: &str, v: &Version, o: &StoreObject, is_latest: bool, url: bool, owner: &str) {
        self.raw("<Version>");
        self.etag(&o.e_tag);
        self.el_bool("IsLatest", is_latest);
        self.el_enc("Key", key, url);
        self.el_timestamp("LastModified", o.last_modified);
        self.raw(owner);
        self.el_int("Size", o.data.size as i64);
        self.storage_class(o);
        self.el("VersionId", &v.id);
        self.raw("</Version>");
    }

    fn delete_marker(&mut self, key: &str, v: &Version, is_latest: bool, url: bool, owner: &str) {
        self.raw("<DeleteMarker>");
        self.el_bool("IsLatest", is_latest);
        self.el_enc("Key", key, url);
        self.el_timestamp("LastModified", v.last_modified);
        self.raw(owner);
        self.el("VersionId", &v.id);
        self.raw("</DeleteMarker>");
    }
}

/// The body being built: the entries are written after `room` spare bytes, which the head of the
/// document (known only once the page is complete: `IsTruncated`, `KeyCount`, the next marker)
/// fills from the right, so nothing is moved.
struct Page {
    xml: Xml,
    room: usize,
}

impl Page {
    fn new(room: usize, cap: usize) -> Self {
        let mut xml = Xml::with_capacity(room + cap);
        xml.buf.resize(room, 0);
        Self { xml, room }
    }

    /// `head` followed by the entries and `tail`. `None` when something in the page can't be
    /// rendered the way s3s would.
    fn finish(mut self, head: &Xml, tail: &[u8]) -> Option<Bytes> {
        self.xml.buf.extend_from_slice(tail);
        if self.xml.bad || head.bad {
            return None;
        }
        let buf = self.xml.buf;
        if head.buf.len() <= self.room {
            let start = self.room - head.buf.len();
            let mut buf = buf;
            buf[start..self.room].copy_from_slice(&head.buf);
            Some(Bytes::from(buf).slice(start..))
        } else {
            let mut out = Vec::with_capacity(head.buf.len() + buf.len() - self.room);
            out.extend_from_slice(&head.buf);
            out.extend_from_slice(&buf[self.room..]);
            Some(out.into())
        }
    }
}

/// An upper estimate of the head's size, for `Page::new`: the fixed elements plus the strings the
/// request echoes (escaping may grow those; the head then is simply copied once).
fn room(echoed: usize, next: usize) -> usize {
    384 + 2 * echoed + 2 * next
}

/// Bytes one `<Contents>` entry takes besides its key, and the key length to plan with: the
/// first key the listing is going to visit.
fn plan(objects: &Index, prefix: &str, max_keys: usize, fixed: usize) -> (usize, usize) {
    use std::ops::Bound;
    let first = objects
        .range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
        .next()
        .filter(|(k, _)| k.starts_with(prefix));
    match first {
        Some((k, _)) => (max_keys.min(objects.len()) * (fixed + k.len()), k.len()),
        None => (0, 0),
    }
}

fn wants_url(e: &Option<EncodingType>) -> bool {
    s3impl::wants_url(e)
}

fn count(page: Option<Bytes>) -> Option<Bytes> {
    let counter = if page.is_some() { &FAST } else { &FALLBACK };
    counter.fetch_add(1, Ordering::Relaxed);
    page
}

// Fixed bytes of one entry: `<Contents>` with its ETag, LastModified, Size and StorageClass,
// plus the owner.
const CONTENTS: usize = 190;
// An estimate: the length of the default account's owner element.
const OWNER_LEN: usize = 64;
const VERSION: usize = 280;

impl FakeS3 {
    pub(crate) fn render_v2(
        &self,
        b: &Bucket,
        input: &ListObjectsV2Input,
        prefix: &str,
        max_keys: usize,
        after: Option<&str>,
    ) -> Option<Bytes> {
        let url = wants_url(&input.encoding_type);
        let owner = input.fetch_owner == Some(true);
        let restore = input
            .optional_object_attributes
            .iter()
            .flatten()
            .any(|a| a.as_str() == OptionalObjectAttributes::RESTORE_STATUS);
        let now = self.clock.now();
        let delimiter = input.delimiter.as_deref().filter(|d| !d.is_empty());
        let objects = b.objects.read();
        let (cap, key_len) = plan(&objects, prefix, max_keys, CONTENTS + if owner { OWNER_LEN } else { 0 });
        let echoed = input.bucket.len()
            + prefix.len()
            + input.continuation_token.as_ref().map_or(0, String::len)
            + input.start_after.as_ref().map_or(0, String::len)
            + delimiter.map_or(0, str::len);
        let mut page = Page::new(room(echoed, key_len * 2), cap);
        let mut prefixes = Xml::with_capacity(0);
        let ListEnd { truncated, next, used } = s3impl::walk_list(
            &objects,
            prefix,
            delimiter,
            after,
            max_keys,
            |key, v, o| {
                let archived = restore
                    .then(|| o.ext.as_deref().and_then(|e| e.storage_class.as_ref()).map(|c| c.as_str()))
                    .flatten()
                    .filter(|c| {
                        matches!(
                            *c,
                            ObjectStorageClass::GLACIER | ObjectStorageClass::DEEP_ARCHIVE | ObjectStorageClass::INTELLIGENT_TIERING
                        )
                    })
                    .and_then(|_| archive::archived(b, key, o, now));
                page.xml
                    .contents(key, o, url, owner.then(|| self.users.owner_xml(v.owner)), archived);
            },
            |p| prefixes.common_prefix(p, url),
        );
        drop(objects);
        page.xml.buf.extend_from_slice(&prefixes.buf);
        page.xml.bad |= prefixes.bad;
        let mut head = Xml::with_capacity(room(echoed, key_len * 2));
        head.raw(DECL);
        head.raw(OPEN);
        head.el("Name", &input.bucket);
        head.el_enc("Prefix", prefix, url);
        head.el_int("MaxKeys", max_keys as i64);
        head.el_int("KeyCount", used as i64);
        if let Some(t) = &input.continuation_token {
            head.el("ContinuationToken", t);
        }
        head.el_bool("IsTruncated", truncated);
        if let Some(n) = &next {
            head.raw("<NextContinuationToken>");
            hex_into(&mut head, n);
            head.raw("</NextContinuationToken>");
        }
        let mut tail = Xml::with_capacity(64);
        if let Some(d) = delimiter {
            tail.el_enc("Delimiter", d, url);
        }
        if let Some(e) = &input.encoding_type {
            tail.el("EncodingType", e.as_str());
        }
        if let Some(s) = &input.start_after {
            tail.el_enc("StartAfter", s, url);
        }
        tail.raw("</ListBucketResult>");
        page.xml.bad |= tail.bad;
        count(page.finish(&head, &tail.buf))
    }

    pub(crate) fn render_v1(
        &self,
        b: &Bucket,
        input: &ListObjectsInput,
        prefix: &str,
        max_keys: usize,
        marker: Option<&str>,
    ) -> Option<Bytes> {
        let url = wants_url(&input.encoding_type);
        let delimiter = input.delimiter.as_deref().filter(|d| !d.is_empty());
        let objects = b.objects.read();
        let (cap, key_len) = plan(&objects, prefix, max_keys, CONTENTS + OWNER_LEN);
        let echoed = input.bucket.len() + prefix.len() + marker.map_or(0, str::len) + delimiter.map_or(0, str::len);
        let mut page = Page::new(room(echoed, 0), cap);
        let mut prefixes = Xml::with_capacity(0);
        let ListEnd { truncated, next, .. } = s3impl::walk_list(
            &objects,
            prefix,
            delimiter,
            marker,
            max_keys,
            |key, v, o| page.xml.contents(key, o, url, Some(self.users.owner_xml(v.owner)), None),
            |p| prefixes.common_prefix(p, url),
        );
        drop(objects);
        page.xml.buf.extend_from_slice(&prefixes.buf);
        page.xml.bad |= prefixes.bad;
        let mut head = Xml::with_capacity(room(echoed, 0));
        head.raw(DECL);
        head.raw(OPEN);
        head.el("Name", &input.bucket);
        head.el_enc("Prefix", prefix, url);
        head.el_enc("Marker", marker.unwrap_or_default(), url);
        head.el_int("MaxKeys", max_keys as i64);
        head.el_bool("IsTruncated", truncated);
        let mut tail = Xml::with_capacity(64 + 2 * key_len);
        if let Some(d) = delimiter {
            tail.el_enc("Delimiter", d, url);
            // NextMarker is only reported when a delimiter was given.
            if let Some(n) = &next {
                tail.el_enc("NextMarker", n, url);
            }
        }
        if let Some(e) = &input.encoding_type {
            tail.el("EncodingType", e.as_str());
        }
        tail.raw("</ListBucketResult>");
        page.xml.bad |= tail.bad;
        count(page.finish(&head, &tail.buf))
    }

    pub(crate) fn render_versions(&self, b: &Bucket, input: &ListObjectVersionsInput, prefix: &str, max_keys: usize) -> Option<Bytes> {
        let url = wants_url(&input.encoding_type);
        let delimiter = input.delimiter.as_deref().filter(|d| !d.is_empty());
        let objects = b.objects.read();
        let (cap, key_len) = plan(&objects, prefix, max_keys, VERSION);
        let echoed = input.bucket.len()
            + prefix.len()
            + delimiter.map_or(0, str::len)
            + input.key_marker.as_ref().map_or(0, String::len)
            + input.version_id_marker.as_ref().map_or(0, String::len);
        let mut page = Page::new(room(echoed, key_len * 2), cap);
        let mut markers = Xml::with_capacity(0);
        let mut prefixes = Xml::with_capacity(0);
        let VersionsEnd {
            truncated,
            next_key,
            next_version_id,
        } = s3impl::walk_versions(
            &objects,
            input,
            prefix,
            max_keys,
            |key, v, is_latest| match &v.obj {
                Some(o) => page.xml.version(key, v, o, is_latest, url, self.users.owner_xml(v.owner)),
                None => markers.delete_marker(key, v, is_latest, url, self.users.owner_xml(v.owner)),
            },
            |p| prefixes.common_prefix(p, url),
        );
        drop(objects);
        // CommonPrefixes, DeleteMarker, then the scalar elements; the Versions follow them.
        let mut head = Xml::with_capacity(room(echoed, key_len * 2) + prefixes.buf.len() + markers.buf.len());
        head.raw(DECL);
        head.raw(OPEN_VERSIONS);
        head.buf.extend_from_slice(&prefixes.buf);
        head.buf.extend_from_slice(&markers.buf);
        head.bad |= prefixes.bad | markers.bad;
        if let Some(d) = delimiter {
            head.el_enc("Delimiter", d, url);
        }
        if let Some(e) = &input.encoding_type {
            head.el("EncodingType", e.as_str());
        }
        head.el_bool("IsTruncated", truncated);
        head.el_enc("KeyMarker", input.key_marker.as_deref().unwrap_or_default(), url);
        head.el_int("MaxKeys", max_keys as i64);
        head.el("Name", &input.bucket);
        if let Some(k) = &next_key {
            head.el_enc("NextKeyMarker", k, url);
        }
        if let Some(v) = &next_version_id {
            head.el("NextVersionIdMarker", v);
        }
        head.el_enc("Prefix", prefix, url);
        head.el("VersionIdMarker", input.version_id_marker.as_deref().unwrap_or_default());
        count(page.finish(&head, b"</ListVersionsResult>"))
    }
}

/// `hex::encode(s)` written into `x`: hex digits need no escaping.
fn hex_into(x: &mut Xml, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    x.buf.reserve(s.len() * 2);
    for &b in s.as_bytes() {
        x.buf.extend_from_slice(&[HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{UserId, Users};

    fn xml(f: impl FnOnce(&mut Xml)) -> String {
        let mut x = Xml::with_capacity(0);
        f(&mut x);
        assert!(!x.bad);
        String::from_utf8(x.buf).unwrap()
    }

    #[test]
    fn owner_matches_the_dto() {
        let users = Users::new("fakes3", "s", &["a<b&c:AK:SK".parse().unwrap()]).unwrap();
        for id in [UserId::DEFAULT, users.resolve(Some("AK"))] {
            let mut buf = Vec::new();
            let mut ser = s3s::xml::Serializer::new(&mut buf);
            ser.content("Owner", &users.owner(id)).unwrap();
            assert_eq!(String::from_utf8(buf).unwrap(), users.owner_xml(id));
        }
    }

    /// The same bytes as s3s' serializer for strings: the five entities and `\r`, nothing else.
    #[test]
    fn text_escapes_like_s3s() {
        let all: String = (0u8..=0x7f).map(char::from).chain("é日本😀".chars()).collect();
        let sample = all.replace(|c: char| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r'), "");
        let mut buf = Vec::new();
        let mut ser = s3s::xml::Serializer::new(&mut buf);
        ser.content("A", sample.as_str()).unwrap();
        assert_eq!(xml(|x| x.el("A", &sample)).as_bytes(), buf);
        // Control characters are flagged rather than written through.
        let mut x = Xml::with_capacity(0);
        x.text("a\u{1}b");
        assert!(x.bad);
    }

    #[test]
    fn url_matches_url_encode() {
        let mut all: Vec<String> = (0u8..=0x7f).map(|b| char::from(b).to_string()).collect();
        all.extend(["", "a b+c/d%2Fe", "é日本😀", "%2F", "/", "//a//", "a%25b", "x*y-z._~"].map(str::to_owned));
        all.push((0u8..=0x7f).map(char::from).collect());
        for s in &all {
            assert_eq!(xml(|x| x.url(s)), s3impl::url_encode(s), "{s:?}");
        }
    }

    #[test]
    fn timestamps_match_s3s() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        const END: u128 = 253_402_300_800_000_000_000;
        let mut cases = vec![
            0u128,
            1,
            999_999,
            1_000_000,
            86_399_999_999_999,
            951_782_400_000_000_000,
            4_102_444_800_999_999_999,
        ];
        cases.extend((0..20_000).map(|_| u128::from(next()) * u128::from(next()) % END));
        cases.push(END - 1);
        for nanos in cases {
            let t = UNIX_EPOCH + std::time::Duration::new((nanos / 1_000_000_000) as u64, (nanos % 1_000_000_000) as u32);
            let mut want = Vec::new();
            Timestamp::from(t).format(TimestampFormat::DateTime, &mut want).unwrap();
            assert_eq!(xml(|x| x.timestamp(t)).as_bytes(), want, "{nanos}");
        }
        let mut x = Xml::with_capacity(0);
        x.timestamp(UNIX_EPOCH + std::time::Duration::from_secs(253_402_300_800));
        assert!(x.bad);
        let mut x = Xml::with_capacity(0);
        x.timestamp(UNIX_EPOCH - std::time::Duration::from_secs(1));
        assert!(x.bad);
    }

    #[test]
    fn ints_match() {
        for n in [0, 1, 9, 10, 99, 100, i64::MAX, -1, -10, i64::MIN] {
            assert_eq!(xml(|x| x.int(n)), n.to_string());
        }
    }

    #[test]
    fn page_head_fills_the_room_or_falls_back() {
        for room in [0, 5, 64] {
            let mut page = Page::new(room, 16);
            page.xml.raw("<a/>");
            let mut head = Xml::with_capacity(0);
            head.raw("<h>");
            assert_eq!(&page.finish(&head, b"</h>").unwrap()[..], b"<h><a/></h>");
        }
        let mut page = Page::new(8, 0);
        let mut head = Xml::with_capacity(0);
        head.raw("<h>");
        page.xml.text("\u{1}");
        assert!(page.finish(&head, b"").is_none());
    }
}
