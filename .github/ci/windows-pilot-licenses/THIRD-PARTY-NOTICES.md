# Third-party distribution notices

This Windows x64 package includes software from the following projects. The
builder's own source is covered by the accompanying [LICENSE](LICENSE) and
[NOTICE](NOTICE).

- The bundled UI's dependency terms are in
  [Angular's build report](licenses/angular-bundle.txt).
- The Rust dependencies resolved for the Windows build are listed in the
  [locked Windows Cargo inventory](licenses/cargo-inventory.txt). Their licenses
  and copyright notices are in `licenses/cargo/`. This inventory includes
  build-time dependencies. The exact crate download links provide source
  for all listed packages, including packages covered by the Mozilla Public
  License 2.0. The [Rust standard library copyright
  record](licenses/rust/COPYRIGHT-library.html) has its license texts in
  `licenses/rust/licenses/`.
- The embedded trust store comes from `webpki-roots` 1.0.11 under
  [CDLA-Permissive-2.0](licenses/cargo/webpki-roots@1.0.11/LICENSE).
  [CCADB Data Usage Terms](https://www.ccadb.org/rootstores/usage#ccadb-data-usage-terms)
- The installer uses the zlib compression profile of
  [NSIS 3.13](licenses/installer/nsis-3.13-COPYING.txt) and the
  [Tauri NSIS plugin 0.5.3](licenses/installer/nsis-tauri-utils-0.5.3-MIT.txt).
  [NSIS source](https://sourceforge.net/projects/nsis/files/NSIS%203/3.13/)
  · [Tauri plugin source](https://github.com/tauri-apps/nsis-tauri-utils/tree/13d9edd27b69310e108d6fbd49f90992f8a05390)
- The desktop executable uses the Microsoft WebView2 SDK loader from
  version 1.0.3800.47. Its
  [license](licenses/installer/Microsoft.Web.WebView2-1.0.3800.47-LICENSE.txt)
  and [notices](licenses/installer/Microsoft.Web.WebView2-1.0.3800.47-NOTICE.txt)
  are supplied under Microsoft's terms. Windows uses the separately installed
  Microsoft Edge WebView2 Evergreen Runtime.
  [Microsoft WebView2 SDK](https://www.nuget.org/packages/Microsoft.Web.WebView2/1.0.3800.47)

Some Cargo archives omit their repository's license file. The companion license
texts come from the matching upstream sources.

- [alloc-stdlib](https://github.com/dropbox/rust-alloc-no-stdlib/blob/0a81fd6928ea3b33c8cd484aa4575d50ffb98012/LICENSE)
- [jsonschema workspace](https://github.com/Stranger6667/jsonschema/tree/0e82636fde8b2afb5e712ede6c261166ea10fc17)
- [Mozilla Public License 2.0 for selectors](https://www.mozilla.org/MPL/2.0/)
- [tauri-plugin](https://github.com/tauri-apps/tauri/blob/59b63ab500da9859f12d8888c3298bc29bba0b92/LICENSE-MIT)
- [simd workspace](https://github.com/Nugine/simd/blob/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE)
- [webview2-rs workspace](https://github.com/wravery/webview2-rs/blob/edc2caf886175ccaebe86078c9cfe1ae2a187328/LICENSE)
