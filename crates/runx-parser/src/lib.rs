//! The pure Rust parser for runx skills, graphs, and tools.

pub mod connector;
pub mod error;
pub mod eval;
pub mod graph;
pub mod harness_fixture;
pub mod install;
mod json_fields;
pub mod mark;
pub mod package;
pub mod packet;
mod registry_ref;
pub mod runner;
pub mod skill;
pub mod tool;
pub mod yaml;

pub use connector::{
    ValidatedConnector, parse_connector_manifest, validate_connector_manifest, whole_match,
};
pub use error::{ParseError, ParseErrorKind, ValidationError, ValidationErrorKind};
pub use eval::{ParserEvalError, ParserEvalOutput, evaluate_parser_document_str};
pub use graph::{
    AgentTask, AskGate, Binding, DuplicatePolicy, ExecutionGraph, GraphGuard, GraphPolicy,
    GraphStep, GraphWhen, LoadStep, LoadTarget, Operand, OperationStepTarget, ProcessTool,
    RawGraphIr, SkillTarget, Source, StepAct, StepKind, ToolTarget, WorkerTool, parse_graph_yaml,
    validate_graph, validate_graph_document, validate_process_tool,
};
pub use harness_fixture::{
    HarnessProviderAccess, HarnessProviderGrantFixture, HarnessProviderHoldFixture,
    HarnessProviderOperationFixture, HarnessProviderResponsesFixture,
};
pub use install::{
    SkillInstallError, SkillInstallOrigin, ValidatedSkillInstall, validate_skill_install,
};
pub use package::{
    PROFILE_FILE, SkillPackageError, SkillPackageSource, ValidatedJavaScriptModule,
    ValidatedPackageTool, ValidatedSkillPackage, javascript_module_imports,
    javascript_process_module_imports, profile_directory, resolve_javascript_module_import,
    validate_skill_package,
};
pub use packet::{
    PACKET_ID_FIELD, PacketSchemaError, ValidatedPacketSchema, parse_packet_schema_document,
};
pub use registry_ref::{REGISTRY_REF_PREFIX, registry_ref};
pub use runner::{
    MarketplaceManifest, RawRunnerManifestIr, SkillRunnerManifest, parse_runner_manifest_yaml,
    validate_runner_manifest,
};
pub use skill::{
    ArtifactPageFraming, ArtifactPageSource, CatalogApproval, CatalogAudience, CatalogCompletion,
    CatalogExecution, CatalogKind, CatalogMetadata, CatalogOperatorReadiness, CatalogRole,
    CatalogSemanticCode, CatalogSemanticDiagnostic, CatalogSemanticReport, CatalogVisibility,
    ConnectorRequirement, HarnessCaller, HarnessExpectation, HarnessHttpExchangeFixture,
    HarnessHttpRequestBodyFixture, HarnessHttpResponseFixture, HarnessMcpCallFixture,
    HarnessProcessCallFixture, InputMode, OperatorJourneyClaim, OperatorJourneyMode, RawSkillIr,
    ReceiptExpectation, RunnerHarnessCase, RunnerHarnessManifest, SkillArtifactContract,
    SkillIdempotencyPolicy, SkillInput, SkillRunnerDefinition, ValidateSkillMode,
    ValidateSkillOptions, ValidatedSkill, analyze_catalog_semantics,
    analyze_package_catalog_semantics, parse_skill_markdown, validate_input_examples,
    validate_skill, validate_skill_artifact_contract, validate_skill_with_options,
};
pub use tool::{
    RawToolManifestIr, ValidatedTool, parse_tool_manifest_json, parse_tool_manifest_yaml,
    validate_tool_manifest,
};
pub use yaml::{
    assert_execution_profile_yaml_subset, assert_yaml_scalar_subset, assert_yaml_supported_subset,
    parse_yaml_document, yaml_scalar_subset_allows,
};
