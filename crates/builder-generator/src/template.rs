use builder_project::ValidatedManifest;

use crate::FileOwner;

pub(crate) const RUNTIME_VERSION: &str = "0.1.0";
pub(crate) const TEMPLATE_VERSION: &str = "1.0.0";

pub(crate) struct RenderedFile {
    pub path: &'static str,
    pub owner: FileOwner,
    pub bytes: Vec<u8>,
}

// The caller serializes the validated portable project separately. Runtime
// source comes directly from the shared crates and an explicit path allowlist.
pub(crate) fn render(_manifest: &ValidatedManifest) -> Vec<RenderedFile> {
    let mut files = Vec::new();
    macro_rules! template {
        ($owner:ident, $($path:literal),+ $(,)?) => {
            $(files.push(file($path, FileOwner::$owner, include_str!(concat!("../template/", $path))));)+
        };
    }
    macro_rules! shared {
        ($crate_path:literal, $($path:literal),+ $(,)?) => {
            $(files.push(file(
                concat!("crates/", $crate_path, "/", $path),
                FileOwner::Generator,
                include_str!(concat!("../../", $crate_path, "/", $path)),
            ));)+
        };
    }

    template!(
        Generator,
        ".gitignore",
        "Cargo.lock",
        "Cargo.toml",
        "README.md",
        "THIRD-PARTY-NOTICES.md",
        "licenses/webpki-roots-1.0.11-CDLA-Permissive-2.0.txt",
        "runtime-config.example.json",
        "src/config.rs",
        "src/main.rs",
        "tests/stdio.rs",
        "crates/mcp-transport/Cargo.toml",
    );
    files.push(file(
        "rust-toolchain.toml",
        FileOwner::Generator,
        include_str!("../../../rust-toolchain.toml"),
    ));
    template!(Project, "src/handlers.rs");
    shared!(
        "builder-project",
        "Cargo.toml",
        "src/document.rs",
        "src/lib.rs",
        "src/parse.rs",
        "src/projection.rs",
        "src/revision.rs",
        "src/scenario.rs",
        "src/syntax.rs",
        "src/validation.rs",
        "src/validation/bindings.rs",
        "src/validation/scenarios.rs",
    );
    shared!(
        "mcp-policy",
        "Cargo.toml",
        "src/grants.rs",
        "src/lib.rs",
        "src/limits.rs",
        "src/paths.rs",
        "src/policy.rs",
        "src/redaction.rs",
        "src/targets.rs",
    );
    shared!(
        "mcp-runtime",
        "Cargo.toml",
        "src/adapter.rs",
        "src/bindings.rs",
        "src/dispatch.rs",
        "src/execution.rs",
        "src/lib.rs",
        "src/pagination.rs",
        "src/prompt.rs",
        "src/registry.rs",
        "src/schema.rs",
        "src/syntax.rs",
        "src/types.rs",
    );
    shared!(
        "mcp-starter-adapters",
        "Cargo.toml",
        "src/filesystem.rs",
        "src/filesystem/native.rs",
        "src/filesystem/native/linux.rs",
        "src/filesystem/native/tests.rs",
        "src/filesystem/native/windows.rs",
        "src/lib.rs",
        "src/rest.rs",
        "src/rest/connection.rs",
        "src/rest/credentials.rs",
        "src/rest/tests.rs",
    );
    shared!(
        "mcp-transport",
        "src/handler.rs",
        "src/lib.rs",
        "src/stdio.rs",
        "src/stdio/frames.rs",
        "src/stdio/frames/tests.rs",
    );
    files
}

fn file(path: &'static str, owner: FileOwner, text: &str) -> RenderedFile {
    RenderedFile {
        path,
        owner,
        // Git may check out text with CRLF. Emit the same bytes on every host.
        bytes: text.replace("\r\n", "\n").into_bytes(),
    }
}
