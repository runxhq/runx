//! Public packets emitted by reusable native runtime capabilities, one module
//! per tool family. Each packet's Rust type is its only source: its schema is
//! generated from it.

mod approval;
mod artifacts;
mod attestation;
mod cli;
mod command;
mod control;
mod data;
mod fs;
mod git;
mod policy;
mod process;
mod receipts;
mod registry;
mod runs;

pub use approval::{ApprovalDecisionActor, ApprovalDecisionPacket, ApprovalDecisionStatus};
pub use artifacts::{LocalArtifact, LocalArtifactPage};
pub use attestation::{
    ATTESTATION_MAX_ACTION_BYTES, ATTESTATION_MAX_CLAIM_BYTES, ATTESTATION_MAX_EVIDENCE,
    ATTESTATION_MAX_LOCATOR_BYTES, ATTESTATION_MAX_PRINCIPAL_BYTES, ATTESTATION_MAX_PROVES_BYTES,
    Attestation, AttestationDecision, AttestationEvidence, AttestationGap,
    AttestationProofBoundary, AttestationSchema, ExternalLedgerNotRequested, ProviderNotCalled,
    RuntimeSealPending,
};
pub use cli::{CLI_HELP_MAX_ARGS, CliHelp, CliHelpFlag};
pub use command::{
    COMMAND_MAX_ENV, COMMAND_MAX_ERRORS, COMMAND_MAX_TIMEOUT_MS, COMMAND_MIN_TIMEOUT_MS,
    COMMAND_OUTPUT_MAX_BYTES, CommandDecision, CommandError, CommandExecution,
    CommandExecutionSchema, CommandOutputMode, CommandPlan, CommandPlanSchema,
    PROCESS_ENV_NAME_PATTERN, ProcessEnvName,
};
pub use control::HarnessContext;
pub use data::{
    DataComparison, DataDigest, DataOperationResult, DataOperationStatus, DataStopCondition,
    DataValidation, DataValidationIssue, DataValueValidation,
};
pub use fs::{
    FS_BUNDLE_MAX_BYTES, FS_BUNDLE_MAX_FILES, FS_LIST_MAX_ENTRIES, FS_SEARCH_MAX_CONTEXT_LINES,
    FS_SEARCH_MAX_MATCHES, FileAppliedDelete, FileAppliedWrite, FileApplyBundle, FileBoundDelete,
    FileBundleDelete, FileBundleWrite, FileDeleteStatus, FileEdit, FileEntryKind,
    FileInverseBundle, FileList, FileListEntry, FileRead, FileReadBundle, FileSearch,
    FileSearchMatch, FileWrite, MissingFile,
};
pub use git::{
    GIT_LOG_MAX_COMMITS, GitBlobDigest, GitBranch, GitCommit, GitDiff, GitLog, GitPatch, GitShow,
    GitStatus,
};
pub use policy::{PolicyLint, PolicyLintEngine, PolicyLintStatus};
pub use process::{
    ProcessExit, ProcessExitReason, ProcessKind, ProcessList, ProcessOutput, ProcessOutputChunk,
    ProcessState, ProcessStream, RunxProcess, TerminalSize,
};
pub use receipts::{
    MatchedReceipt, RECEIPT_TOOL_MAX_RECEIPTS, ReceiptProof, ReceiptProofDecision,
    ReceiptProofSchema, ReceiptProofStore, ReceiptProofVerification, ReceiptQuery,
    ReceiptQuerySchema, ReceiptQuerySource, ReceiptQueryVerification,
};
pub use registry::{
    CatalogHit, CatalogPackageKind, CatalogRunner, CatalogSource, ConnectorInspection,
    InspectedOperation, RegistryInspection, RegistrySearchPacket,
};
pub use runs::{RUN_LIST_MAX_RUNS, RunList, RunListEntry, RunListSchema};
