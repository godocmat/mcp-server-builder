use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use builder_app::{
    ManifestRevision, ProjectLocation, ProjectRecovery, ProjectService, ProjectSnapshot, Reply,
    project_read_draft_file, project_validate_root,
};
use clap::{ArgGroup, Args, Subcommand};

use crate::{failure, finish, project_service};

#[derive(Subcommand)]
pub enum Action {
    /// List projects registered in the local catalog.
    List,
    /// Create a schema 1 project in an empty destination.
    Create {
        root: PathBuf,
        #[arg(long)]
        name: String,
    },
    /// Open a saved project and report its current revision.
    Open { root: PathBuf },
    /// Validate a portable project without opening or running it.
    Validate { root: PathBuf },
    /// Save a validated draft if the current revision matches.
    Save {
        root: PathBuf,
        /// Path to the edited manifest.
        #[arg(long)]
        from: PathBuf,
        /// Revision from project open or project create.
        #[arg(long)]
        expect: ManifestRevision,
    },
    /// Restore a reviewed backup or recover a draft into a new folder.
    Recover(RecoveryArgs),
}

#[derive(Args)]
#[command(group(ArgGroup::new("source").required(true).args(["from_revision", "draft"])))]
pub struct RecoveryArgs {
    root: PathBuf,
    /// Revision of the reviewed last valid backup.
    #[arg(long, requires = "current")]
    from_revision: Option<ManifestRevision>,
    /// Expected revision of the manifest currently on disk.
    #[arg(long, group = "current", requires = "from_revision")]
    expect: Option<ManifestRevision>,
    /// Require the current manifest to be missing before restoring the backup.
    #[arg(long, group = "current", requires = "from_revision")]
    expect_missing: bool,
    /// Manifest to preserve as a new project.
    #[arg(long, requires = "out")]
    draft: Option<PathBuf>,
    /// Destination folder for a recovered draft.
    #[arg(long, requires = "draft")]
    out: Option<PathBuf>,
}

pub fn run(action: Action, profile: Option<PathBuf>, json: bool) -> io::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(execute(action, project_service(profile), json))
}

async fn execute(action: Action, service: ProjectService, json: bool) -> io::Result<ExitCode> {
    match action {
        Action::List => finish("project.list", &service.list().await, json, |projects| {
            let mut stdout = io::stdout().lock();
            if projects.is_empty() {
                writeln!(stdout, "No projects are registered.")?;
            }
            for project in projects {
                writeln!(
                    stdout,
                    "{}  {:?}  {}",
                    project.name, project.status, project.root_display_path
                )?;
            }
            Ok(())
        }),
        Action::Create { root, name } => finish(
            "project.create",
            &service.create(root, name).await,
            json,
            write_snapshot,
        ),
        Action::Open { root } => finish(
            "project.open",
            &service.open(ProjectLocation::Root(root)).await,
            json,
            write_snapshot,
        ),
        Action::Validate { root } => finish(
            "project.validate",
            &project_validate_root(root).await,
            json,
            |_| writeln!(io::stdout().lock(), "Project is valid."),
        ),
        Action::Save { root, from, expect } => {
            let reply = match project_read_draft_file(from).await {
                Reply::Success { data, .. } => {
                    service
                        .save(ProjectLocation::Root(root), expect, data)
                        .await
                }
                Reply::Failure { error, diagnostics } => Reply::Failure { error, diagnostics },
            };
            finish("project.save", &reply, json, write_snapshot)
        }
        Action::Recover(args) => {
            let reply = recover(&service, args).await;
            finish("project.recover", &reply, json, write_snapshot)
        }
    }
}

async fn recover(service: &ProjectService, args: RecoveryArgs) -> Reply<ProjectSnapshot> {
    let source = match args {
        RecoveryArgs {
            from_revision: Some(revision),
            expect,
            ..
        } => ProjectRecovery::Backup {
            revision,
            expected_revision: expect,
        },
        RecoveryArgs {
            draft: Some(draft),
            out: Some(destination),
            ..
        } => match project_read_draft_file(draft).await {
            Reply::Success { data, .. } => ProjectRecovery::Draft {
                document: Box::new(data),
                destination,
            },
            Reply::Failure { error, diagnostics } => return Reply::Failure { error, diagnostics },
        },
        _ => {
            return failure(
                builder_app::DiagnosticCode::CliUsage,
                "Recovery requires a backup revision or a draft and destination.",
                "Run mcp-builder project recover --help for usage.",
            );
        }
    };
    service
        .recover(ProjectLocation::Root(args.root), source)
        .await
}

fn write_snapshot(snapshot: &ProjectSnapshot) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(
        stdout,
        "{}  {}",
        snapshot.document.name, snapshot.root_display_path
    )?;
    writeln!(stdout, "Revision  {}", snapshot.revision)
}
