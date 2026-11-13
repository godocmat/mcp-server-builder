# Standalone components and terms

## Bundled source

| Component | Version | Role |
| --- | --- | --- |
| Standalone scaffolding | 1.0.0 (Cargo package 0.1.0) | Startup, operator configuration, handler assembly and tests |
| `builder-project` | 0.1.0 | Portable manifest parsing, validation and runtime projection |
| `mcp-policy` | 0.1.0 | Access checks, execution limits and redaction |
| `mcp-runtime` | 0.1.0 | Capability discovery and execution |
| `mcp-starter-adapters` | 0.1.0 | REST and read-only file handlers |
| `mcp-transport` | 0.1.0 | Stdio transport for MCP 2026-07-28 and 2025-11-25 |

The project proposes Apache-2.0 for the builder-owned bundled runtime and
scaffolding. The owners' approval of the generated-code and runtime terms and
the redistribution review are pending. This inventory records proposed terms
and third-party provenance. Project inputs and customer-authored sources follow
their owners' terms.

## Third-party inputs

Cargo resolves the registry packages below using the version and registry
checksum in `Cargo.lock`. They include target-specific, build and test
dependencies. Cargo fetches their source at build time. License expressions
come from each pinned package's published manifest. Review the original
license and notice files for the target and binaries before distribution.

The TLS trust bundle in `webpki-roots` 1.0.11 derives its data from the Common
CA Database. Its published terms are included with this export.
[CDLA-Permissive-2.0](licenses/webpki-roots-1.0.11-CDLA-Permissive-2.0.txt) ·
[CCADB Data Usage Terms](https://www.ccadb.org/rootstores/usage#ccadb-data-usage-terms)

| Registry package | Version | Declared terms |
| --- | --- | --- |
| `ahash` | `0.8.12` | `MIT OR Apache-2.0` |
| `aho-corasick` | `1.1.5` | `Unlicense OR MIT` |
| `allocator-api2` | `0.2.21` | `MIT OR Apache-2.0` |
| `android_system_properties` | `0.1.6` | `MIT OR Apache-2.0` |
| `atomic-waker` | `1.1.2` | `Apache-2.0 OR MIT` |
| `autocfg` | `1.5.1` | `Apache-2.0 OR MIT` |
| `aws-lc-rs` | `1.19.0` | `ISC AND (Apache-2.0 OR ISC)` |
| `aws-lc-sys` | `0.47.0` | `ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)` |
| `base64` | `0.23.1` | `MIT OR Apache-2.0` |
| `bit-set` | `0.8.0` | `Apache-2.0 OR MIT` |
| `bit-vec` | `0.8.0` | `Apache-2.0 OR MIT` |
| `bitflags` | `2.13.2` | `MIT OR Apache-2.0` |
| `block-buffer` | `0.12.1` | `MIT OR Apache-2.0` |
| `borrow-or-share` | `0.2.4` | `MIT-0` |
| `bumpalo` | `3.20.3` | `MIT OR Apache-2.0` |
| `bytecount` | `0.6.9` | `Apache-2.0/MIT` |
| `bytes` | `1.12.1` | `MIT` |
| `cc` | `1.11.1` | `MIT OR Apache-2.0` |
| `cfg-if` | `1.0.5` | `MIT OR Apache-2.0` |
| `chrono` | `0.4.45` | `MIT OR Apache-2.0` |
| `cmake` | `0.1.58` | `MIT OR Apache-2.0` |
| `const-oid` | `0.10.3` | `Apache-2.0 OR MIT` |
| `core-foundation-sys` | `0.8.7` | `MIT OR Apache-2.0` |
| `cpufeatures` | `0.3.1` | `MIT OR Apache-2.0` |
| `crypto-common` | `0.2.2` | `MIT OR Apache-2.0` |
| `data-encoding` | `2.11.1` | `MIT` |
| `digest` | `0.11.3` | `MIT OR Apache-2.0` |
| `displaydoc` | `0.2.7` | `MIT OR Apache-2.0` |
| `dunce` | `1.0.5` | `CC0-1.0 OR MIT-0 OR Apache-2.0` |
| `dyn-clone` | `1.0.20` | `MIT OR Apache-2.0` |
| `email_address` | `0.2.9` | `MIT` |
| `equivalent` | `1.0.3` | `Apache-2.0 OR MIT` |
| `errno` | `0.3.15` | `MIT OR Apache-2.0` |
| `fancy-regex` | `0.20.2` | `MIT` |
| `fastrand` | `2.5.0` | `Apache-2.0 OR MIT` |
| `find-msvc-tools` | `0.1.15` | `MIT OR Apache-2.0` |
| `fluent-uri` | `0.4.1` | `MIT` |
| `foldhash` | `0.2.0` | `Zlib` |
| `form_urlencoded` | `1.2.2` | `MIT OR Apache-2.0` |
| `fraction` | `0.17.0` | `MIT OR Apache-2.0` |
| `fs_extra` | `1.3.0` | `MIT` |
| `futures` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-channel` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-core` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-executor` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-io` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-macro` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-sink` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-task` | `0.3.34` | `MIT OR Apache-2.0` |
| `futures-util` | `0.3.34` | `MIT OR Apache-2.0` |
| `getrandom` | `0.2.17` | `MIT OR Apache-2.0` |
| `getrandom` | `0.3.4` | `MIT OR Apache-2.0` |
| `getrandom` | `0.4.3` | `MIT OR Apache-2.0` |
| `hashbrown` | `0.17.1` | `MIT OR Apache-2.0` |
| `heck` | `0.5.0` | `MIT OR Apache-2.0` |
| `http` | `1.5.0` | `MIT OR Apache-2.0` |
| `http-body` | `1.1.0` | `MIT` |
| `http-body-util` | `0.1.5` | `MIT` |
| `httparse` | `1.10.1` | `MIT OR Apache-2.0` |
| `hybrid-array` | `0.4.15` | `MIT OR Apache-2.0` |
| `hyper` | `1.12.0` | `MIT` |
| `hyper-util` | `0.1.21` | `MIT` |
| `iana-time-zone` | `0.1.65` | `MIT OR Apache-2.0` |
| `iana-time-zone-haiku` | `0.1.2` | `MIT OR Apache-2.0` |
| `icu_collections` | `2.4.0` | `Unicode-3.0` |
| `icu_locale_core` | `2.4.0` | `Unicode-3.0` |
| `icu_normalizer` | `2.4.0` | `Unicode-3.0` |
| `icu_normalizer_data` | `2.4.0` | `Unicode-3.0` |
| `icu_properties` | `2.4.0` | `Unicode-3.0` |
| `icu_properties_data` | `2.4.0` | `Unicode-3.0` |
| `icu_provider` | `2.4.0` | `Unicode-3.0` |
| `idna` | `1.1.0` | `MIT OR Apache-2.0` |
| `idna_adapter` | `1.2.2` | `Apache-2.0 OR MIT` |
| `indexmap` | `2.14.2` | `Apache-2.0 OR MIT` |
| `ipnet` | `2.12.2` | `MIT OR Apache-2.0` |
| `itoa` | `1.0.19` | `MIT OR Apache-2.0` |
| `jobserver` | `0.1.35` | `MIT OR Apache-2.0` |
| `js-sys` | `0.3.107` | `MIT OR Apache-2.0` |
| `jsonschema` | `0.61.2` | `MIT` |
| `jsonschema-regex` | `0.61.2` | `MIT` |
| `jsonschema-value` | `0.61.2` | `MIT` |
| `libc` | `0.2.191` | `MIT OR Apache-2.0` |
| `linux-raw-sys` | `0.12.1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `litemap` | `0.8.3` | `Unicode-3.0` |
| `lock_api` | `0.4.14` | `MIT OR Apache-2.0` |
| `log` | `0.4.35` | `MIT OR Apache-2.0` |
| `memchr` | `2.8.3` | `Unlicense OR MIT` |
| `micromap` | `0.3.0` | `MIT` |
| `mime` | `0.3.18` | `MIT OR Apache-2.0` |
| `mio` | `1.2.4` | `MIT` |
| `num` | `0.4.3` | `MIT OR Apache-2.0` |
| `num-bigint` | `0.4.8` | `MIT OR Apache-2.0` |
| `num-cmp` | `0.1.0` | `MIT/Apache-2.0` |
| `num-complex` | `0.4.6` | `MIT OR Apache-2.0` |
| `num-integer` | `0.1.47` | `MIT OR Apache-2.0` |
| `num-iter` | `0.1.46` | `MIT OR Apache-2.0` |
| `num-rational` | `0.4.2` | `MIT OR Apache-2.0` |
| `num-traits` | `0.2.19` | `MIT OR Apache-2.0` |
| `once_cell` | `1.21.4` | `MIT OR Apache-2.0` |
| `outref` | `0.5.2` | `MIT` |
| `parking_lot` | `0.12.5` | `MIT OR Apache-2.0` |
| `parking_lot_core` | `0.9.12` | `MIT OR Apache-2.0` |
| `pastey` | `0.2.3` | `MIT OR Apache-2.0` |
| `percent-encoding` | `2.3.2` | `MIT OR Apache-2.0` |
| `pin-project-lite` | `0.2.17` | `Apache-2.0 OR MIT` |
| `pkg-config` | `0.3.34` | `MIT OR Apache-2.0` |
| `potential_utf` | `0.1.6` | `Unicode-3.0` |
| `proc-macro2` | `1.0.107` | `MIT OR Apache-2.0` |
| `quote` | `1.0.47` | `MIT OR Apache-2.0` |
| `r-efi` | `5.3.0` | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` |
| `r-efi` | `6.0.0` | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` |
| `redox_syscall` | `0.5.18` | `MIT` |
| `ref-cast` | `1.0.27` | `MIT OR Apache-2.0` |
| `ref-cast-impl` | `1.0.27` | `MIT OR Apache-2.0` |
| `referencing` | `0.61.2` | `MIT` |
| `regex` | `1.13.1` | `MIT OR Apache-2.0` |
| `regex-automata` | `0.4.18` | `MIT OR Apache-2.0` |
| `regex-syntax` | `0.8.11` | `MIT OR Apache-2.0` |
| `ring` | `0.17.14` | `Apache-2.0 AND ISC` |
| `rmcp` | `3.9.0` | `Apache-2.0` |
| `rustix` | `1.1.6` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `rustls` | `0.23.46` | `Apache-2.0 OR ISC OR MIT` |
| `rustls-pki-types` | `1.16.0` | `MIT OR Apache-2.0` |
| `rustls-webpki` | `0.103.17` | `ISC` |
| `rustversion` | `1.0.23` | `MIT OR Apache-2.0` |
| `schemars` | `1.2.3` | `MIT` |
| `schemars_derive` | `1.2.3` | `MIT` |
| `scopeguard` | `1.2.0` | `MIT OR Apache-2.0` |
| `semver` | `1.0.28` | `MIT OR Apache-2.0` |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_core` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_derive` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_derive_internals` | `0.30.0` | `MIT OR Apache-2.0` |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` |
| `serde_path_to_error` | `0.1.20` | `MIT OR Apache-2.0` |
| `sha2` | `0.11.1` | `MIT OR Apache-2.0` |
| `shlex` | `2.0.1` | `MIT OR Apache-2.0` |
| `slab` | `0.4.12` | `MIT` |
| `smallvec` | `1.16.5` | `MIT OR Apache-2.0` |
| `socket2` | `0.6.5` | `MIT OR Apache-2.0` |
| `stable_deref_trait` | `1.2.1` | `MIT OR Apache-2.0` |
| `strum` | `0.28.0` | `MIT` |
| `strum_macros` | `0.28.0` | `MIT` |
| `subtle` | `2.6.1` | `BSD-3-Clause` |
| `syn` | `2.0.119` | `MIT OR Apache-2.0` |
| `syn` | `3.0.10` | `MIT OR Apache-2.0` |
| `synstructure` | `0.14.0` | `MIT` |
| `tempfile` | `3.27.0` | `MIT OR Apache-2.0` |
| `thiserror` | `2.0.21` | `MIT OR Apache-2.0` |
| `thiserror-impl` | `2.0.21` | `MIT OR Apache-2.0` |
| `tinystr` | `0.8.5` | `Unicode-3.0` |
| `tinyvec` | `1.13.3` | `Zlib OR Apache-2.0 OR MIT` |
| `tokio` | `1.54.1` | `MIT` |
| `tokio-macros` | `2.7.2` | `MIT` |
| `tokio-rustls` | `0.26.6` | `MIT OR Apache-2.0` |
| `tokio-util` | `0.7.20` | `MIT` |
| `tracing` | `0.1.44` | `MIT` |
| `tracing-attributes` | `0.1.31` | `MIT` |
| `tracing-core` | `0.1.36` | `MIT` |
| `try-lock` | `0.2.5` | `MIT` |
| `typenum` | `1.20.1` | `MIT OR Apache-2.0` |
| `unicode-general-category` | `1.1.0` | `Apache-2.0` |
| `unicode-ident` | `1.0.27` | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `unicode-normalization` | `0.1.26` | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `untrusted` | `0.9.0` | `ISC` |
| `url` | `2.5.8` | `MIT OR Apache-2.0` |
| `utf8_iter` | `1.1.0` | `Apache-2.0 OR MIT` |
| `utf_types` | `1.0.0` | `Apache-2.0 OR MIT` |
| `uuid` | `1.29.2` | `Apache-2.0 OR MIT` |
| `uuid-simd` | `0.8.0` | `MIT` |
| `version_check` | `0.9.5` | `MIT/Apache-2.0` |
| `vsimd` | `0.8.0` | `MIT` |
| `want` | `0.3.2` | `MIT` |
| `wasi` | `0.11.1+wasi-snapshot-preview1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `wasip2` | `1.0.4+wasi-0.2.12` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `wasm-bindgen` | `0.2.130` | `MIT OR Apache-2.0` |
| `wasm-bindgen-macro` | `0.2.130` | `MIT OR Apache-2.0` |
| `wasm-bindgen-macro-support` | `0.2.130` | `MIT OR Apache-2.0` |
| `wasm-bindgen-shared` | `0.2.130` | `MIT OR Apache-2.0` |
| `webpki-roots` | `1.0.11` | `CDLA-Permissive-2.0` |
| `windows-core` | `0.62.2` | `MIT OR Apache-2.0` |
| `windows-implement` | `0.60.2` | `MIT OR Apache-2.0` |
| `windows-interface` | `0.59.3` | `MIT OR Apache-2.0` |
| `windows-link` | `0.2.1` | `MIT OR Apache-2.0` |
| `windows-result` | `0.4.1` | `MIT OR Apache-2.0` |
| `windows-strings` | `0.5.1` | `MIT OR Apache-2.0` |
| `windows-sys` | `0.52.0` | `MIT OR Apache-2.0` |
| `windows-sys` | `0.61.2` | `MIT OR Apache-2.0` |
| `windows-targets` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_aarch64_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_aarch64_msvc` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_i686_gnu` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_i686_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_i686_msvc` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_x86_64_gnu` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_x86_64_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` |
| `windows_x86_64_msvc` | `0.52.6` | `MIT OR Apache-2.0` |
| `wit-bindgen` | `0.57.1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `writeable` | `0.6.4` | `Unicode-3.0` |
| `yoke` | `0.8.4` | `Unicode-3.0` |
| `yoke-derive` | `0.8.4` | `Unicode-3.0` |
| `zerocopy` | `0.8.63` | `BSD-2-Clause OR Apache-2.0 OR MIT` |
| `zerocopy-derive` | `0.8.63` | `BSD-2-Clause OR Apache-2.0 OR MIT` |
| `zerofrom` | `0.1.8` | `Unicode-3.0` |
| `zerofrom-derive` | `0.1.8` | `Unicode-3.0` |
| `zeroize` | `1.9.1` | `Apache-2.0 OR MIT` |
| `zerotrie` | `0.2.5` | `Unicode-3.0` |
| `zerovec` | `0.11.9` | `Unicode-3.0` |
| `zerovec-derive` | `0.11.7` | `Unicode-3.0` |
| `zmij` | `1.0.23` | `MIT` |
