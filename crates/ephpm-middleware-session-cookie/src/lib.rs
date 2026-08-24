//! `session-cookie` — the hot-path **verifier** half of the ePHPm GitHub OAuth
//! preview-access gate, shipped as a `dlopen`'d native middleware module.
//!
//! It runs on every request (the PHP path and, since ePHPm #408, the
//! static-file path too), verifies the HS256 session cookie that `github-auth`
//! (or any external identity service holding the same secret) minted, and
//! redirects unauthenticated browsers to the login URL. It never talks to the
//! identity provider — verification is one local HMAC.
//!
//! Pair it with `github-auth` from this same workspace: mount `github-auth`
//! first (it owns the login/callback paths and issues the cookie) and this
//! module second (it verifies the cookie on every request and redirects to
//! `github-auth`'s `login_path` when it is absent or invalid). The two share
//! the HMAC secret (`session_secret` ↔ `secret`) and the cookie name
//! (`cookie_name` ↔ `cookie`, whose defaults already match).
//!
//! The middleware implementation lives in [`session_cookie`]; this file only
//! wires the C ABI exports via [`ephpm_middleware::declare!`].
#![allow(unsafe_code)] // the declare! glue is FFI; every unsafe block carries a note.

mod hs256;
pub mod session_cookie;

pub use session_cookie::SessionCookie;

ephpm_middleware::declare!(SessionCookie);
