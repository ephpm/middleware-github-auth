# `ephpm-middleware-github-auth`

A GitHub OAuth **login gate** for [ePHPm](https://github.com/ephpm/ephpm),
shipped as a `dlopen`'d native middleware module. It is the **cold path** of the
ePHPm preview-access gate (ephpm#487/#491/#499): it answers *"does the person at
this browser have access, on GitHub, to the repo this preview is for?"* —
**once, at login** — and then issues a stateless signed HS256 session. The
per-request **hot path** (verifying that session on every subsequent request) is
a separate, GitHub-free verifier that ships **inside** the ePHPm binary (the
`preview-gate` builtin); this module never verifies a session, and never calls
GitHub on a request that already has one.

This lives in its own repository, separate from ePHPm core, on purpose:

- It is **preview-site infrastructure**, not a general ePHPm product deliverable.
- It is **stable** — it changes far less often than core.
- It carries an **HTTP client and a TLS stack** (to reach `github.com`) that
  have no business inside the `ephpm` binary. Keeping it out of the workspace
  keeps that weight — and its build/CI — off core.

## How the host loads it

This is not a builtin. The `ephpm` binary has no static registry entry named
`github-auth`; the module must be present on disk as a shared object and mounted
as a **global** `[[middleware]]`:

```toml
[[middleware]]
library = "github-auth"
order   = 10
config  = { client_id = "Iv1.…", client_secret = "env:GH_CLIENT_SECRET",
            session_secret = "env:EPHPM_SESSION_SECRET", repo = "acme/web" }
```

For a bare name (`library = "github-auth"`) the host loader
(`resolve_library` in ePHPm's `crates/ephpm-server/src/middleware.rs`) searches
the current directory, `$EPHPM_MIDDLEWARE_DIR`, and
`/usr/local/lib/ephpm/middleware`, trying these file names in order:

1. `github-auth.<os>-<arch>.<ext>` — e.g. `github-auth.linux-x86_64.so`
2. `libgithub-auth.<ext>`
3. `github-auth.<ext>`

The published **release asset is named for form (1)**:
`github-auth.linux-x86_64.so`. (The cargo artifact is `libgithub_auth.so`, with
an underscore, because the crate's `[lib] name` is `github_auth`; the release
workflow renames it.) You can also point `library` at an explicit path
(`library = "/opt/ephpm/modules/github-auth.linux-x86_64.so"`), which the loader
uses as-is.

### Getting the `.so`

**From a release (recommended):** download `github-auth.linux-x86_64.so` and
`SHA256SUMS` from this repo's [Releases](../../releases), verify, and place it
where the loader looks:

```bash
sha256sum -c SHA256SUMS
install -Dm755 github-auth.linux-x86_64.so /usr/local/lib/ephpm/middleware/github-auth.linux-x86_64.so
```

**Building it yourself** (needs a glibc-dynamic ePHPm and a matching source
tag — the C ABI is only stable within one host **major** version):

```bash
cargo build --release
# target/release/libgithub_auth.so  →  rename to github-auth.linux-x86_64.so
```

## The ABI contract (why the git-rev pin is load-bearing)

The `ephpm-middleware` crate — the C ABI, the `Middleware` trait, the `declare!`
macro, and the host callback table — **stays in ePHPm core**. This repo depends
on it by **git `rev`**, litewire-style, and never vendors it. The FFI boundary
between this `.so` and the host is `EphpmHostV1` / `ABI_V1`; a *fork* of that
type is a fork of the contract and would be silent undefined behaviour. The pin
is currently ePHPm **v0.11.1** (`292e74f5…`), which carries ABI **major 1 /
minor 4**.

Compatibility is gated on the **major** byte only: `declare!` refuses to `init`
unless `(host.abi_version >> 24) == 1`. Minor versions are additive, so a host
newer than the pinned minor still loads the module; the per-preview repository
channel this module reads (`Request::gate_repo()`, minor 4) simply falls back
closed on an older host.

To rebuild against a newer host ABI: bump `rev` in `Cargo.toml`, run
`cargo update`, and re-release.

## The OAuth App callback URL

The module's default endpoints live under `/_ephpm/auth/`:

- **Login:** `/_ephpm/auth/github/login`
- **Callback:** `/_ephpm/auth/github/callback`

The ePHPm router routes the reserved `/_ephpm/auth/` sub-namespace **to the
middleware chain** (the rest of `/_ephpm/` is server-internal). Register the
callback as the GitHub OAuth App's **Authorization callback URL**:

```
https://<host>/_ephpm/auth/github/callback
```

### One App across a `*.preview` wildcard fleet (the apex flow)

A GitHub OAuth App allows exactly **one** callback host — not a wildcard. So a
preview fleet funnels every callback through one fixed **apex** host and carries
the target preview in the signed OAuth `state`:

```toml
[[middleware]]
library = "github-auth"
order   = 10
config  = { client_id = "Iv1.…", client_secret = "env:GH_CLIENT_SECRET",
            session_secret = "env:EPHPM_SESSION_SECRET",
            redirect_uri  = "https://preview.example.com/_ephpm/auth/github/callback",
            cookie_domain = ".preview.example.com",
            sites = { "pr-1.preview.example.com" = { repo = "acme/web" } } }
```

Register `https://preview.example.com/_ephpm/auth/github/callback` (the apex) as
the App's callback URL. Login runs on the target subdomain; the callback lands on
the apex, reads the signed `state`, runs the **target's** authz check, mints a
session whose `site` claim is the **target**, sets it with
`Domain=.preview.example.com`, and `302`s back to the target. A domain-wide
cookie is safe here only because the session verifier honours the `site` binding
(ephpm#396): the cookie travels the fleet but verifies on exactly one preview.

## Three things to know before reading the code

1. **This is the cold path only.** It issues sessions; it never verifies one. A
   request carrying the session cookie gets `CONTINUE` and the `preview-gate`
   verifier decides. **Mounted alone it is not an authenticator** — it logs that
   at startup.
2. **No GitHub call ever happens on a request that has a session.** That is
   structural: `GithubAuth::route` is a pure function, only its `Callback`
   variant reaches `github.rs`, and only the exact configured `callback_path`
   produces it. `tests/oauth_round_trip.rs` asserts the stub GitHub's request
   counter does not move across 50 authenticated requests.
3. **It is a `cdylib`, not a builtin, on purpose.** It needs an HTTP client and
   TLS; none of that is in the `ephpm` binary. The module links exactly one
   crypto provider, `aws-lc-rs` — see the `rustls` dependency comment in
   `Cargo.toml`, which is load-bearing.

## Layout

| file | what it holds |
|---|---|
| `src/lib.rs` | routing (`Route`), the login/callback/bypass flows, `declare!` |
| `src/config.rs` | strict, fail-closed config parsing; `env:` secret indirection |
| `src/github.rs` | the outbound half: TLS, token exchange, the three access checks |
| `src/token.rs` | HS256 minting, key derivation, constant-time comparison |
| `src/redirect.rs` | return-to validation (the open-redirect defence) |
| `src/cookie.rs` | cookie reading and `Set-Cookie` construction |
| `tests/oauth_round_trip.rs` | the whole flow against a stub GitHub over real sockets |

## Building & testing

```bash
cargo build --release          # produces target/release/libgithub_auth.so
cargo test                     # unit + the oauth_round_trip integration test
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt --all -- --check
```

CI (GitHub-hosted runners) builds the `linux-x86_64-gnu` cdylib, runs the tests,
and — on a `v*` tag — publishes `github-auth.linux-x86_64.so` + `SHA256SUMS` as
release assets. See `.github/workflows/`.

What is **not** covered, and needs a human: a round trip against the real
`github.com` with a registered GitHub App. Everything this side of that is
exercised over real TCP against a stub.

## Operator documentation

Full operator docs for the preview gate (both halves) live in ePHPm core:
**[GitHub OAuth Gate (native middleware)](https://github.com/ephpm/ephpm/blob/main/site/content/guides/github-auth-middleware.md)**.
