# middleware-github-auth

A **GitHub-OAuth preview-access gate** for [ePHPm](https://github.com/ephpm/ephpm),
as native middleware. It puts a private preview site (a per-PR WordPress
preview, a staging Laravel app) behind GitHub login: *does the person at this
browser have access, on GitHub, to the repo this preview is for?* — answered
**once**, at login, then carried in a stateless signed session that every later
request is judged by locally.

This repo is **source that the preview build compiles in** — see
[Distribution](#distribution). It is not a released binary and has no release
pipeline.

## The two modules

The gate is a pair, split so a network call can never land on the hot path:

| Crate | Role | Runs on | Talks to GitHub |
|-------|------|---------|-----------------|
| [`ephpm-middleware-github-auth`](crates/ephpm-middleware-github-auth) | **Issuer** (cold path) — runs the OAuth dance, checks access, mints the session cookie | login, callback, and any request with no session cookie | yes, ~3 calls per login |
| [`ephpm-middleware-session-cookie`](crates/ephpm-middleware-session-cookie) | **Verifier** (hot path) — verifies the session cookie, redirects to login when it is absent or invalid | every request | **never** |

Mount the issuer first (it owns the login/callback paths), the verifier second.
Three things line up between them: the HMAC key (`session_secret` ↔ `secret`),
the cookie name (`cookie_name` ↔ `cookie`, whose defaults already match), and
the login path (`login_path` ↔ `login_url`). Mounted **alone the issuer is not
an authenticator** — it stops a request with no cookie but does not judge one
that has a cookie; it logs that at startup. Always mount the verifier.

```toml
# Issuer — owns /_ephpm/auth/github/{login,callback}, issues the session.
[[middleware]]
library = "/opt/ephpm/modules/libgithub_auth.so"
order   = 10
config  = { client_id = "Iv1.…", client_secret = "env:GH_CLIENT_SECRET",
            session_secret = "env:EPHPM_SESSION_SECRET", repo = "acme/web" }

# Verifier — checks the cookie on every request, redirects to login otherwise.
[[middleware]]
library = "/opt/ephpm/modules/libsession_cookie.so"
order   = 20
config  = { secret = "env:EPHPM_SESSION_SECRET", cookie = "ephpm_session",
            login_url = "/_ephpm/auth/github/login", return_to_param = "next" }
```

## The OAuth flow

| Request | Verdict | Cost |
|---------|---------|------|
| no session cookie | `302` → GitHub `authorize`, `Set-Cookie` signed `state` | no network |
| `GET {callback_path}?code=…&state=…` | verify `state`, exchange the code, check access, `302` → return-to with `Set-Cookie` session | 3 GitHub calls |
| `GET {login_path}` | `302` → GitHub `authorize` | no network |
| valid bypass token, no cookie | `302` → same path with `Set-Cookie` session | no network |
| session cookie present | verifier checks it: `CONTINUE`, or `302` → login | no network |

The issued session is a **compact HS256 JWT** carrying the GitHub login, the
vhost it was issued for, how it was obtained, and an expiry — no server-side
session record, so restarts and second nodes log nobody out. The issuer's three
GitHub calls per login are: `POST /login/oauth/access_token` (code → token),
`GET /user` (the identity), and the access check (`GET /repos/{owner}/{name}`,
`/user/memberships/orgs/{org}`, or a team-membership call).

**Access is unrevocable until it expires** — that is the cost of a
self-contained token. `session_ttl_secs` (default 8h) is therefore the blast
radius of a stolen cookie; rotating `session_secret` is the only early
revocation, and it logs everyone out at once.

## Configuration

Issuer (`ephpm-middleware-github-auth`), key knobs — full validation and
defaults are in [`config.rs`](crates/ephpm-middleware-github-auth/src/config.rs):

| key | default | meaning |
|-----|---------|---------|
| `client_id` | **required** | OAuth app / GitHub App client id |
| `client_secret` | **required** | client secret — use `env:NAME`, never a literal |
| `session_secret` | **required, ≥32 bytes** | HMAC key for the session token; must match the verifier's `secret` |
| `repo` / `org` / `team` | one is required | the access target: read access to `owner/name`, active org membership, or active team membership |
| `sites` | unset | per-vhost table mapping each preview hostname to its own target (authoritative when present) |
| `session_ttl_secs` | `28800` | session lifetime (60 … 604800) |
| `bypass_token` | unset | pre-shared token (≥32 bytes) letting CI reach a preview headlessly; presenting it mints a normal session |
| `github_base` / `github_api_base` | `https://github.com` / `https://api.github.com` | override for GitHub Enterprise Server |
| `scopes` | empty | OAuth scopes; empty is correct for a GitHub App |

Allowed users are expressed as GitHub *relationships*, not a user list: anyone
who can read `repo` (or is an active member of `org` / `team`) is allowed. A
GitHub App is the recommended shape — a classic OAuth App needs the broad `repo`
scope to see a private repository at all, which is why `scopes` defaults to
empty.

Verifier (`ephpm-middleware-session-cookie`): `secret` (**required**, matches
the issuer's `session_secret`), `login_url` (**required**, the issuer's
`login_path`), `cookie` (`ephpm_session`), `return_to_param`, `site_param`,
`issuer`, `audience`, `claims_header`.

## It gates static assets too (ePHPm #408 / #395)

Since ePHPm [#408](https://github.com/ephpm/ephpm/pull/408) the middleware
request phase runs on the **static-file path** as well as the PHP path, and it
fails closed — the chain is evaluated *before* the file on disk is opened. Both
modules gate on path/query/cookie alone and are blind to whether the target is a
PHP script or a static byte, so an unauthenticated request for `/assets/app.js`
(or a private PDF, or any file under the document root) gets the same login
redirect and **the bytes are never served**. That is the
[#395](https://github.com/ephpm/ephpm/issues/395) fix: before it, an auth gate
protected only PHP-dispatched requests and leaked static assets. Each module has
a test pinning it (`the_gate_denies_a_static_asset_before_it_is_read` /
`a_static_asset_with_no_session_is_denied_before_it_is_read`).

## Distribution

There is **none** — this repo has no release workflow, no prebuilt `.so`, no
checksums or manifest. It is source the **preview build compiles in** (the
`switchboard` preview control plane and the `wordpress-sample` PR-preview app)
via a git dependency / vendored source and a `[[middleware]]` mount. The
official in-tree modules (`jwt`, `cors`, `ratelimit`, …) ship inside the ePHPm
binary; this gate is preview-only infrastructure and lives here instead.

The trade-off of the `dlopen`'d cdylib form: a fully static (musl) ePHPm cannot
`dlopen`, so it cannot load these; the stock glibc-dynamic Linux release can. A
musl build would need them compiled in as builtins.

## ABI pin

Both modules build against the `ephpm-middleware` ABI as a **git dependency
pinned by `rev`** to ePHPm `main` (see the workspace
[`Cargo.toml`](Cargo.toml)) — the same way ePHPm pins litewire. A drift in
`EphpmHostV1` / `ABI_V1` would be silent UB at the FFI boundary, so the pin is
exact. The pinned rev is past #408 (the static-path request phase) and does not
depend on the request-scheme accessor proposed in #409 (not merged).

## Build & test

```bash
cargo build --workspace --release   # → target/release/lib{github_auth,session_cookie}.so
cargo test --workspace              # unit tests + the OAuth round trip vs a stub GitHub
cargo clippy --workspace --all-targets -- -D warnings
cargo +nightly fmt --all -- --check
```

CI (`.github/workflows/ci.yml`) runs fmt / clippy / test / release-build on
every PR and push to `main`, on GitHub-hosted runners.

What CI cannot cover, and needs a human: a round trip against the real
`github.com` with a registered GitHub App. Everything this side of that is
exercised over real TCP against a stub in
[`oauth_round_trip.rs`](crates/ephpm-middleware-github-auth/tests/oauth_round_trip.rs).
