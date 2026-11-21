# Entry-layer Rust unit tests

These files test the crate's command and orchestration layer. `lib.rs` declares each
as a direct child module with `#[cfg(test)]` and `#[path = "tests/…"]`. Keeping that
parent preserves access through `super` without making production internals public.
Module-local tests in other production modules stay beside those modules.

Run the full library regression from the repository root:

```sh
CARGO_BUILD_JOBS=2 cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1
```

**Run serially.** Credential generations and the identity lease are process-global;
independent tests deliberately change them. Multiple harness threads can interfere
with another test's fixture or leave a loopback mock waiting for a request that was
correctly rejected. This requirement predates moving the test files; the identity
race tests also document it. Tests still exercise their own asynchronous races.

Default ignored tests require real local WeChat assets. Do not run them as part of
ordinary structural regression or enable them just to change a test count.

## Ownership

- `runtime`, `reconcile_owner`, `connector_gate`, `manual_upload`, `security_identity`
  and `logout_queue`: lifecycle, configuration, ownership and interruption boundaries.
- `upload_package`, `upload_coverage`, `upload_list`, `cloud_retry`, `legacy_key_scan`,
  `message_paging` and `attachment`: package construction, progress, recovery and paging.
- `key_flow`: key import, extractor locations and clearing decrypted snapshots.
- `devify_error`, `space` and `id_list`: response parsing and small boundary helpers.

The former `key_flow_tests` mixed upload packaging and key import. Upload tests now
use the `upload_package_tests::…` filter; key tests keep `key_flow_tests::…`.
`connector_gate` reads the same `lib.rs` through `include_str!("../lib.rs")`.

Keep each handwritten test file around or below 1000 lines and group tests by the
behavior they verify. Split unrelated responsibilities before a file becomes hard
to review; do not shorten assertions or compress code to meet a line count. Keep
fixture data synthetic, temporary and scoped to the test's account/service owner.
