# Where the small-request regression comes from

Each feature branch merged since `e0ce5c0` was A/B-tested alone and showed ≤ ±2 %. Interleaved against
`e0ce5c0`, current main (`c03ea07`) is slower on small requests by [run 41259058518](https://github.com/AkaraChen/fakes3/actions/runs/41259058518):
GET 4 KiB +11.6 % server CPU/op, HEAD +8.5 %, PUT +5.0 %, versioned +5.7 %. This document says where that
comes from, what each part costs, and what removing it would take.

(Written in English because the task was; the other docs in this directory are Chinese.)

## Short answer

* The regression is **user-space CPU only**: kernel time and syscalls per request did not go up (they go
  *down* in B under load, see below). It is about **1.3–1.7 µs per request on a fast runner** (GET 19.9 → 21.3 µs),
  about 2.5 µs on the runners the earlier runs landed on (runner speed varies 2.5× between CI VMs, the *ratio* is stable).
* It is **not one thing**. It is the sum of ~8 independent per-request costs, each 0.1–0.4 µs, each added by a
  different feature branch, none big enough to show in that branch's own A/B. Cumulative: +9 allocations/request
  (34.5 → 42.5 for GET), +2 entries in the response `HeaderMap`, ~14 hashed `HeaderMap` lookups by custom
  `x-amz-*` names, 3 more clock reads, 1 more mutex acquisition.
* Removing exactly four things — the service-wrapper extras (request log, request ids, digest/clock checks), the
  access hook, the host parser, the object-extension checks in the handlers — brings main back to
  **1.00–1.01 × `e0ce5c0`** ([run 41268888054](https://github.com/AkaraChen/fakes3/actions/runs/41268888054), `exp/attr-all`). Nothing else matters for small GET/HEAD/PUT.
* Behaviour-preserving fixes (prototype `perf/hot-path`) recover **~2.5 % of the ~10 %** in auth mode
  (1.2–3.8 % per workload) and **1–4 %** in the default (no-auth) mode (noisier). The rest is either inherent to required S3 behaviour (request-id
  headers, the authorization hook) or a feature that would have to become opt-in (request log).

## Method

* **Cumulative curve**: 16 interleaved A/Bs, A = `e0ce5c0`, B = one first-parent commit of main each (ab.yml, 8 rounds,
  `auth=1`: both sides verify signatures). Then seven adjacent-hop A/Bs to pin the steps.
* **Profile on CI** (`.github/workflows/profile.yml`, `bench/profile.py`; the workflow runs on pushes to
  `exp/attr-profile*` because a workflow that is not on the default branch cannot be dispatched): builds both refs with
  `line-tables-only`, fat LTO, and frame pointers (`-C force-frame-pointers`, `CFLAGS=-fno-omit-frame-pointer`), runs
  warp 4 KiB get / put / stat against each on the same runner, and records
  (1) unprofiled server user/sys CPU per request (3 runs each, ABBA), (2) syscalls, context switches and
  **allocator calls** per request (uprobes on `mi_malloc_aligned` & co), (3) `perf record -a` flat profiles at 1999 Hz
  (system-wide, filtered by pid — per-task sampling costs a timer reprogram per context switch, which in a VM
  made the server 2.5× slower and invalidated the first attempt), (4) frame-pointer call-graph profiles. Every address is
  expanded with `addr2line -i`, so inlined frames are attributed to the *source file and call-site line* they came
  from, and the shares are scaled by the unprofiled CPU per request. Artifacts per run: `report.md`, JSON,
  folded stacks, flamegraphs (`*-A.svg`, `*-B.svg`, `*-diff.svg`).
* **Experiments**: `exp/attr-*` branches off main that remove one cost (correctness irrelevant), A/B against main
  with ab.yml (`auth=1`, 8 rounds unless stated).
* Noise: each ab.yml point has ±1–1.5 % from run to run and ±3 % from build to build (code layout; e.g. the S3 Select commit
  changes no hot-path code and still reads +0.9 %/−0.5 %). Read steps, not single points.

## 1. Cumulative curve

B/A **server µs/op** (A = `e0ce5c0`; > 1 is slower), 8 rounds, signatures verified on both sides.

| commit | what | small_get | small_get_c1 | stat | small_put | run |
|---|---|---:|---:|---:|---:|---|
| `f6de556` | lib split + clock | 0.996 | 1.016 | 0.998 | 1.001 | [41264454132](https://github.com/AkaraChen/fakes3/actions/runs/41264454132) |
| `8605fec` | memory limit | 0.985 | 0.990 | 1.015 | 1.013 | [41264459719](https://github.com/AkaraChen/fakes3/actions/runs/41264459719) |
| `26e53bf` | control plane | 1.036 | 1.024 | 1.029 | 1.015 | [41264466237](https://github.com/AkaraChen/fakes3/actions/runs/41264466237) |
| `05d7fde` | bucket semantics | 1.048 | 1.038 | 1.052 | 1.037 | [41264472605](https://github.com/AkaraChen/fakes3/actions/runs/41264472605) |
| `c4f04fa` | distribution | 1.056 | 1.064 | 1.050 | 1.027 | [41264478395](https://github.com/AkaraChen/fakes3/actions/runs/41264478395) |
| `317f66a` | object semantics | 1.057 | 1.054 | 1.074 | 1.038 | [41264484266](https://github.com/AkaraChen/fakes3/actions/runs/41264484266) |
| `acbb21a` | bucket defaults | 1.046 | 1.038 | 1.069 | 1.033 | [41264489814](https://github.com/AkaraChen/fakes3/actions/runs/41264489814) |
| `c9befcf` | no-auth rewriting | 1.074 | 1.042 | 1.050 | 1.024 | [41264495574](https://github.com/AkaraChen/fakes3/actions/runs/41264495574) |
| `d736d97` | archive tiers | 1.061 | 1.055 | 1.083 | 1.048 | [41264500921](https://github.com/AkaraChen/fakes3/actions/runs/41264500921) |
| `3187044` | no-auth default, identity | 1.004 | 1.056 | 1.009 | 1.044 | [41264506616](https://github.com/AkaraChen/fakes3/actions/runs/41264506616) |
| `8edf2cb` | request ids + cheap fixes | 1.025 | 1.116 | 1.090 | 1.056 | [41264511998](https://github.com/AkaraChen/fakes3/actions/runs/41264511998) |
| `bc340d2` | S3 Select | 1.116 | 1.081 | 1.147 | 1.066 | [41264517198](https://github.com/AkaraChen/fakes3/actions/runs/41264517198) |
| `9b3793f` | fast list XML | 1.072 | 1.138 | 1.034 | 1.049 | [41264522296](https://github.com/AkaraChen/fakes3/actions/runs/41264522296) |
| `c8f7d87` | multi-user | 1.116 | 1.085 | 1.134 | 1.059 | [41264527516](https://github.com/AkaraChen/fakes3/actions/runs/41264527516) |
| `a2bb035` | s3s workarounds | 1.080 | 1.110 | 1.058 | 1.052 | [41264532264](https://github.com/AkaraChen/fakes3/actions/runs/41264532264) |
| `c03ea07` | remaining fixes (= main) | 1.110 | 1.088 | 1.131 | 1.045 | [41264538119](https://github.com/AkaraChen/fakes3/actions/runs/41264538119) |

(The absolute µs/op of A varies 17–57 µs between rows: different runner CPUs. Only the ratio within a row means anything.)

Single points wiggle by ±3 %, so adjacent hops (A = previous step, B = next, 8 rounds, small_get and stat):

| hop | small_get | stat | run | reading |
|---|---:|---:|---|---|
| `8605fec` → `26e53bf` control plane | 1.031 | 1.023 | [41270647450](https://github.com/AkaraChen/fakes3/actions/runs/41270647450) | **step ≈ +2.5 %**: request log, clock reads |
| `26e53bf` → `05d7fde` bucket semantics | 1.012 | 1.023 | [41270651406](https://github.com/AkaraChen/fakes3/actions/runs/41270651406) | **step ≈ +1.5 %**: access hook, host parser, CORS check |
| `05d7fde` → `317f66a` distribution + object semantics | 1.013 | 1.002 | [41270654906](https://github.com/AkaraChen/fakes3/actions/runs/41270654906) | ≈ +0.5 % (handler checks) |
| `317f66a` → `3187044` defaults, no-auth, archive | 0.991 | 1.001 | [41270658760](https://github.com/AkaraChen/fakes3/actions/runs/41270658760) | ≈ 0 (the no-auth path is not in an `auth=1` run) |
| `3187044` → `8edf2cb` request ids | 1.018 | 1.019 | [41270662487](https://github.com/AkaraChen/fakes3/actions/runs/41270662487) | **step ≈ +1.8 %**: request-id headers, SSE header rejection |
| `8edf2cb` → `bc340d2` S3 Select | 1.009 | 0.995 | [41270666149](https://github.com/AkaraChen/fakes3/actions/runs/41270666149) | noise (no hot-path code changed) |
| `bc340d2` → `c03ea07` list XML, multi-user, workarounds | 1.002 | 1.007 | [41270669741](https://github.com/AkaraChen/fakes3/actions/runs/41270669741) | ≈ +0.5 % |

The hops add to +7.6 % (GET) / +7.0 % (HEAD), the single-ended curve says +11 %/+13 %; the difference is the noise
of the individual points. Three steps (control plane, bucket semantics, request ids) explain ≥ 80 %; the commits
that are large in the diff (Select, list XML, archive, multi-user) cost nothing measurable on small requests.

## 2. Profile: what changed per request

[Profile run 41267308208](https://github.com/AkaraChen/fakes3/actions/runs/41267308208), fast runner (A = 19.9 µs/GET), `e0ce5c0` vs `c03ea07`, 16 connections:

| | GET A | GET B | PUT A | PUT B | HEAD A | HEAD B |
|---|---:|---:|---:|---:|---:|---:|
| server user µs/op | 9.91 | 11.65 (**+1.74**) | 23.27 | 25.14 (**+1.87**) | 8.41 | 9.58 (**+1.17**) |
| server sys µs/op | 9.99 | 9.63 (−0.36) | 15.28 | 15.62 | 8.09 | 7.69 (−0.40) |
| total | 19.89 | 21.28 (+7.0 %) | 38.56 | 40.76 (+5.7 %) | 16.51 | 17.27 (+4.6 %) |

The regression is entirely in user space. Under load B makes *fewer* syscalls and context switches per
request (GET: 3.19 → 3.00 syscalls, 0.55 → 0.42 switches) because slower request handling lets more
connections be ready per `epoll_wait`; that batching hides part of the user-space cost in "server µs/op". With one connection
(small_get_c1, no batching) the full cost shows: +12 %.

**Allocator calls per request** ([profile run 41271273150](https://github.com/AkaraChen/fakes3/actions/runs/41271273150), uprobes on `mi_malloc_aligned`/`mi_realloc_aligned`):

| | A | B | Δ |
|---|---:|---:|---:|
| GET malloc / realloc | 34.5 / 1.0 | 42.5 / 2.0 | **+8.0 / +1.0** |
| HEAD malloc / realloc | 33.4 / 1.0 | 41.7 / 2.0 | **+8.3 / +1.0** |
| PUT malloc / realloc | 57.2 / 3.0 | 64.3 / 3.0 | **+7.1 / 0** |

The +8 are: the boxed per-request future in `lib.rs` (1), the request log's `String` (1), `HeaderName::from_bytes` of
`x-amz-request-id` and `x-amz-id-2` (2, `h.insert("…", v)` with a `&str`), their two `HeaderValue`s (2), the response
`HeaderMap` growing past its 6 entries (1 + the realloc), `async_trait`'s box for the access check (1).

**Per-contributor user-space cost**, µs per request on the fast runner, from call-graph profiles (inclusive of
everything the frame calls or inlines: allocator, `HeaderMap` hashing, clock reads), `e0ce5c0` → `c03ea07`:

| contributor | where | GET | HEAD | PUT | % of GET Δ | nature |
|---|---|---:|---:|---:|---:|---|
| request log: `start` + `finish` | `reqlog.rs`, `lib.rs:332,394` | 0.17 + 0.25 | 0.13 + 0.25 | 0.10 + 0.28 | 30 % | alloc `String` (freed on another thread when the ring drops it), 3 clock reads, 3 hashed header lookups, mutex + `fetch_add` on shared lines, 250-byte record |
| request-id headers | `reqid.rs::decorate` | 0.35 | 0.29 | 0.27 | 25 % | 2 custom-name allocs, 2 value allocs, 2 FNV hashes, `HeaderMap` grows 8→16 slots (rehash), base64 of 48 bytes |
| SSE write-header rejection on reads | `objext::reject_write_headers` | 0.27 | 0.34 | – | 19 % | **4 hashed `&str` lookups of 28–47-byte names** per GET/HEAD |
| access hook | `access.rs::check` + s3s glue | 0.15 | 0.13 | 0.15 | 11 % | `async_trait` box, 2 hashed lookups (`x-amz-expected-bucket-owner`, `…source…`), identity |
| empty `Content-MD5` check | `digest::empty_content_md5` | 0.035 | 0.037 | 0.046 | 2.5 % | 1 hashed lookup |
| `s3s_compat` (`Date`→`x-amz-date`, unsized check) | `s3s_compat.rs`, `sigv2::kind` | 0.04 | 0.035 | 0.06 | 3 % | header presence checks |
| server clock reads | `clock.rs` | 0.02 | 0.03 | 0.08 | 2 % | vDSO `clock_gettime` |
| rest (host parse, `Store::bucket` clone, PUT: `Bucket::put` +0.11, `etag_for` +0.06, `sink`, memory reservation) | | ~0.1 | ~0 | ~0.3 | | |
| **sum / measured user Δ** | | **1.3 / 1.74** | **1.24 / 1.17** | **1.0 + 0.3 / 1.87** | | overlaps and sampling noise ±0.2 |

Two cross-cutting facts behind the numbers:

* `http::HeaderMap` hashes a custom header name (every `x-amz-*`) with **byte-wise FNV** — one dependent multiply per byte —
  and a `&str` key is additionally lower-cased and validated (`parse_hdr`) on every call. `FnvHasher::write`, `hash_elem_using`
  and `parse_hdr` together are the single largest function-level delta (+0.25 µs inclusive in GET) and they are
  spread over ~14 call sites per request (log ×3, access ×2, digest, SSE ×4, request id ×4).
* The response `HeaderMap` that s3s builds has 5–6 entries and the map's capacity is 6 (8 slots × ¾). The two
  request-id headers make it the 7th and 8th → one grow-and-rehash on every response.

## 3. Experiments (ab.yml, `auth=1`, B = main + one removal; server µs/op, B/A)

| branch | removes | small_get | get_c1 | stat | put | run |
|---|---|---:|---:|---:|---:|---|
| `exp/attr-reqlog` | request log (`request_log: false`) | 0.976 | 0.964 | 0.982 | 0.990 | [41266534676](https://github.com/AkaraChen/fakes3/actions/runs/41266534676) |
| `exp/attr-reqid` | `id.decorate` (no id headers) | 0.974 | 0.991 | 0.978 | 0.991 | [41266538550](https://github.com/AkaraChen/fakes3/actions/runs/41266538550) |
| `exp/attr-logid` | both | 0.986 | 0.945 | 0.977 | 0.962 | [41268903235](https://github.com/AkaraChen/fakes3/actions/runs/41268903235) |
| `exp/attr-access` | no `set_access` | 0.981 | 0.991 | 0.980 | 0.991 | [41266542182](https://github.com/AkaraChen/fakes3/actions/runs/41266542182) |
| `exp/attr-host` | no `set_host` (virtual-host parsing) | 0.999 | 0.997 | 0.985 | 0.997 | [41266545980](https://github.com/AkaraChen/fakes3/actions/runs/41266545980) |
| `exp/attr-objext` | SSE/lock/archive/expiration/ACL work in GET/HEAD/PUT | 0.997 | 0.999 | 0.993 | 1.008 | [41267351270](https://github.com/AkaraChen/fakes3/actions/runs/41267351270) |
| `exp/attr-mem` | memory-limit atomics | 0.996 | 0.990 | 1.004 | 0.995 | [41268907333](https://github.com/AkaraChen/fakes3/actions/runs/41268907333) |
| `exp/attr-lib` | everything in the service closure except `merge_duplicate_headers` + `s3.call` | **0.944** | **0.958** | **0.924** | **0.947** | [41267347756](https://github.com/AkaraChen/fakes3/actions/runs/41267347756) |
| `exp/attr-all` vs **`e0ce5c0`** | lib + access + host + objext together | **1.009** | **1.014** | **1.001** | **0.972** | [41268888054](https://github.com/AkaraChen/fakes3/actions/runs/41268888054) |

Reruns with 16 rounds (small_get / stat) of the three small ones, because their first points were close to the noise:

| branch | small_get | stat | run |
|---|---:|---:|---|
| `exp/attr-objext` | 0.990 | 0.980 | [41272652899](https://github.com/AkaraChen/fakes3/actions/runs/41272652899) |
| `exp/attr-access` | 0.989 | 0.982 | [41272660842](https://github.com/AkaraChen/fakes3/actions/runs/41272660842) |
| `exp/attr-host` | 0.998 | 1.018 | [41272656829](https://github.com/AkaraChen/fakes3/actions/runs/41272656829) |

so: handler object-extension work ≈ 1–2 %, access hook ≈ 1–2 %, host parsing ≈ 0.

Reading: the wrapper (request log ≈ 2 %, request ids ≈ 2 %, the rest of the closure ≈ 2–3 %), the access hook (≈ 1.5 %), the
handler-level SSE rejection (≈ 1 %) and the host parser (≈ 0–0.5 %) add up to the regression; memory accounting is ≤ 0.5 % and PUT only.
The "rest of the closure" is more than its profile (0.25 µs) suggests: it includes the `Box::pin`'d per-request future
(an extra allocation plus a copy of a ~1 KB state machine) and the `dyn` indirection, which have no function of their own to show up under.
`exp/attr-all` closing the gap to `e0ce5c0` is the check that nothing else is hiding.

## 4. Options, ranked by what they would give back

Savings are server CPU per request, GET at 19.9 µs/21.3 µs; "%" is of main's cost.

| # | contributor | saving | proposed fix | trade-off |
|---|---|---|---|---|
| 1 | request log (≈ 0.4 µs, 2 %) | 0.15–0.2 µs (prototype) → 0.4 µs if off | (a) inline target / access key in the record (no `String`, no cross-thread free), static header names, one header scan — **done in `perf/hot-path`**. (b) make recording opt-in (`--request-log`) | (a) none. (b) **behaviour change**: tests that call `server.requests()` / `ctl requests` would have to enable it; on by default today and documented that way |
| 2 | request-id headers (≈ 0.35 µs, 1.7 %) | 0.1 µs (prototype) | static names, one `entry()` instead of `contains_key` + `insert` — **done**. The remaining ~0.25 µs is two value allocations, two FNV hashes and the response map's grow+rehash | **inherent** to sending `x-amz-request-id` / `x-amz-id-2` on every response (SDKs read both; dropping a field is off the table). An s3s change that pre-sizes the response `HeaderMap` (16 slots) would remove the grow; per-thread arena for the two values would save ~0.05 µs more |
| 3 | access hook (≈ 0.15–0.4 µs, 1–2 %) | 0.05 µs (prototype: one header scan instead of 2 hashed lookups) | to remove the rest, build two `S3Service`s from the same `FakeS3` — one without `set_access`, used when the request is signed, no `x-amz-*expected-bucket-owner` header, single account and no bucket policy anywhere (exactly the conditions under which `check` returns `Ok` after one atomic load today) | none in behaviour; one extra service object and a per-request branch. Not prototyped. `async_trait`'s box is s3s' |
| 4 | SSE write-header rejection on GET/HEAD (0.27 µs by profile, ≤ 0.7 % by ab) | 0.25 µs | one pass over the request's headers for the common prefix instead of four hashed lookups — **done** | none |
| 5 | rest of the closure (digest check, clock, `s3s_compat`, form check, ~0.2 µs, 1 %) | 0.05 µs | `Content-MD5` presence from the same header scan — **done**; the rest is a handful of atomic loads and header checks, each already cheap | none |
| 6 | host parser (`--domain localhost` default, ≈ 0.1 µs, 0.5 %) | 0.1 µs | default `domains` to empty (virtual-host addressing only when asked) | **behaviour change**: `bucket.localhost:9000` stops working without `--domain localhost` |
| 7 | `merge_duplicate_headers` (in `e0ce5c0` too, 0.1 µs) | 0.1 µs, a *gain* over `e0ce5c0` | detect a repeated name by looking for a name that follows itself in `HeaderMap::iter` instead of hashing every name to count its values — **done** | none |
| 8 | PUT only: `Bucket::put` `Arc<Version>`, `etag_for`, memory reservation (≈ 0.3 µs) | – | needed for snapshots / lock-free reads / memory limit | inherent to those features |

Inherent to required S3 behaviour: request-id headers (2), authorization (3: ACL/policy/BPA need a hook; the
fast path is already one atomic load), the SSE-on-read rejection (4: S3 answers 400), the `Date`/`x-amz-date` and
unsized-body repairs (5). Not required by S3, so the only candidates for "opt-in": the request log (1b), virtual-host
domains (6).

## 5. `perf/hot-path`: behaviour-preserving prototype

Two commits on `perf/hot-path` (not merged): `src/names.rs` (static `HeaderName`s, one `scan` of the request headers for
`content-md5`, `x-amz-copy-source`, `x-amz-decoded-content-length`, `x-amz-*expected-bucket-owner`), reqlog without per-request
allocation, `reject_write_headers` in one pass, `merge_duplicate_headers` without hashing, the default-mode `noauth`
lookups. No behaviour changes (no field dropped, same defaults). `cargo test` (unit + all integration tests) passes and
`cargo clippy --lib --bins` is clean on the branch; new unit test for `names::scan`, the `reqlog` tests were adapted to the new `start` signature.

Measured with ab.yml (server µs/op, B/A, 10 rounds; v2 = `31dd775`; v1 = `e208e43`, the first two commits):

| comparison | small_get | get_c1 | stat | put | versioned | run |
|---|---:|---:|---:|---:|---:|---|
| v2 **vs main**, signatures verified | **0.969** | **0.982** | **0.962** | **0.988** | 0.986 | [41270610713](https://github.com/AkaraChen/fakes3/actions/runs/41270610713) |
| v2 **vs `e0ce5c0`**, signatures verified | 1.084 | 1.101 | 1.074 | 1.034 | 1.068 | [41270614482](https://github.com/AkaraChen/fakes3/actions/runs/41270614482) |
| v2 vs main, **default mode** (`auth=0`) | 0.991 | 0.979 | 0.991 | 0.989 | – | [41272649281](https://github.com/AkaraChen/fakes3/actions/runs/41272649281) |
| v1 vs main, signatures verified | 1.001 | 0.990 | 0.972 | 0.992 | 0.986 | [41268859552](https://github.com/AkaraChen/fakes3/actions/runs/41268859552) |
| v1 vs `e0ce5c0` | 1.089 | 1.073 | 1.107 | 1.043 | 1.085 | [41268863503](https://github.com/AkaraChen/fakes3/actions/runs/41268863503) |
| v1 vs main, default mode | 0.957 | 0.978 | 0.954 | 0.986 | 0.991 | [41268867270](https://github.com/AkaraChen/fakes3/actions/runs/41268867270) |

(Main's own gap to `e0ce5c0` in the same measurement style is 1.116 / 1.124 / 1.085 / 1.050 / 1.057, [41259058518](https://github.com/AkaraChen/fakes3/actions/runs/41259058518);
runs on different runners differ by ±3 %, so read the table as "recovers 2–4 % of ~10 %", v1→v2 differences are inside the noise.)

Profile of v2 vs main ([run 41272664475](https://github.com/AkaraChen/fakes3/actions/runs/41272664475)): allocator calls per request GET 43.0 → 39.7,
PUT 64.4 → 61.3, HEAD 41.6 → 38.5 (−3.1 of the +8); user CPU −1.4 µs/GET on a 56 µs runner (≈ −0.45 µs on the fast one);
the SSE rejection (−0.58 µs of 0.67 on that runner), the two `&str` lookups in the access check, the
`Content-MD5` lookup and the request-id `HeaderName` allocations are gone; request log and ids shrink by ~35 %.

What is left after v2, from experiments on top of it (A = `perf/hot-path`, 10 rounds):

| removed on top of v2 | small_get | get_c1 | stat | put | run |
|---|---:|---:|---:|---:|---|
| the whole request log (`exp/attr-hp-nolog`) | 0.993 | 0.993 | 0.985 | 0.993 | [41272808053](https://github.com/AkaraChen/fakes3/actions/runs/41272808053) |
| the log's lock + ring (record built, not stored; `exp/attr-hp-nolock`) | 0.997 | 0.997 | 0.997 | 1.000 | [41272811699](https://github.com/AkaraChen/fakes3/actions/runs/41272811699) |
| the request-id headers (`exp/attr-hp-noid`) | 0.985 | 0.988 | 0.983 | 0.999 | [41272816056](https://github.com/AkaraChen/fakes3/actions/runs/41272816056) |

So the log costs ≈ 1 % and the ids ≈ 1.5 % after the fixes (from ≈ 2 % and ≈ 2.5 %); the log's mutex is not the problem (0.3 %).
Making the log opt-in would now give back ≈ 1 %, not 2 %. The remaining ≈ 7 % of the gap is the access hook (≈ 1.5 %), the
boxed per-request future and other closure work (≈ 2–3 %), the 8 → 5 extra allocations, request ids (inherent), and layout noise.

Not prototyped, with estimates: two `S3Service`s so that the access hook is skipped under exactly the conditions where it
returns `Ok` after one atomic load (−0.5 to −1 %); replacing the boxed future in `lib.rs` by a hand-written `Future` enum (−0.5 to −1 %,
uncertain); a per-thread arena for the two request-id values (−0.2 %).

## 6. Branches and artifacts

Pushed to origin and kept: `perf/attribution` (this document, `bench/profile.py`, `.github/workflows/profile.yml`),
`perf/hot-path` (the prototype). Nothing is merged and nothing was pushed to main.

Experiment branches, **deleted from origin at the end of this work** (each is main plus a one-line removal; recreate with the
commit message of the branch if needed): `exp/attr-reqlog`, `exp/attr-reqid`, `exp/attr-logid`, `exp/attr-access`, `exp/attr-host`,
`exp/attr-lib`, `exp/attr-objext`, `exp/attr-mem`, `exp/attr-all`, `exp/attr-hp-nolog`, `exp/attr-hp-nolock`, `exp/attr-hp-noid`,
and the profile trigger branches `exp/attr-profile2` … `exp/attr-profile5` (a push to `exp/attr-profile*` runs `profile.yml`
with the settings in `bench/profile.env` of that branch; a plain `workflow_dispatch` works once the workflow is on main).

Profile runs (artifacts: `report.md`, per-profile JSON, folded stacks, flamegraphs; 90 days): e0ce5c0 vs main
[41267308208](https://github.com/AkaraChen/fakes3/actions/runs/41267308208) and [41271273150](https://github.com/AkaraChen/fakes3/actions/runs/41271273150) (with allocation counts);
main vs hot-path [41268936741](https://github.com/AkaraChen/fakes3/actions/runs/41268936741) and [41272664475](https://github.com/AkaraChen/fakes3/actions/runs/41272664475);
e0ce5c0 vs hot-path [41270230427](https://github.com/AkaraChen/fakes3/actions/runs/41270230427).

To reproduce a profile: `git push -f <branch with bench/profile.env> origin HEAD:exp/attr-profile9`, with e.g.
`BASE=e0ce5c0`, `HEAD_REF=main`, `AUTH=1`, `WORKLOADS=get,put,stat` in `bench/profile.env`.

## Caveats

* CI runners are shared VMs with CPUs that differ 2.5× in speed; every comparison here is interleaved on one runner.
* Profile attribution is sampling-based (1999 Hz, ~15–30 k samples/run): per-contributor numbers are ±0.1–0.2 µs, which
  is why the experiments (ab.yml) are the arbiter. SSE rejection is the example where they disagree (profile 0.27 µs, ab ≤ 0.7 %).
* The default (no-auth) mode cannot be A/B'd against `e0ce5c0`: its `--no-auth` has no accept-any-signature path
  (answers 501 to signed requests), so `ab.yml auth=0` with `e0ce5c0` as A fails. Default-mode numbers are main vs `perf/hot-path` only.
