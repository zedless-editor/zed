# Zedless

This is Zedless, a fork of Zed that's designed to be privacy-friendly and local-first.

Zedless is currently work-in-progress. Feel free to contribute!

---

### Planned Changes from Upstream

This is a list of things that Zedless will do differently.

- No reliance on proprietary cloud services
  - Components and features that strictly rely on non-selfhostable cloud services will be removed.
- No spyware
  - Telemetry and automatic crash reporting will be removed.
- Priority on bringing your own infrastructure
  - Any feature that makes use of a network service will allow you to configure which provider to use in a standard format, e.g. by specifying the base URL of an API.
  - Any such feature will not have a list of "default providers".
  - Any such feature will be disabled by default.
- No CLA
  - Contributors' copyright shall not be reassigned.
  - No rugpulls.

### Licensing

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).
