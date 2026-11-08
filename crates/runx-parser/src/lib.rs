//! The pure Rust parser for runx packages, connectors and tools.

pub mod connector;
pub mod data;
pub mod error;
pub mod eval;
pub mod graph;
pub mod install;
mod json_fields;
pub mod manifest;
pub mod mark;
pub mod package;
pub mod packet;
mod registry_ref;
pub mod skill;
pub mod test_fixture;
pub mod tool;
pub mod yaml;

pub use connector::{
    ValidatedConnector, parse_connector_manifest, validate_connector_manifest, whole_match,
};
pub use error::{ParseError, ValidationError};
pub use eval::{ParserEvalError, ParserEvalOutput, evaluate_parser_document_str};
pub use graph::{
    AgentTask, AskGate, Binding, LoadStep, LoadTarget, Operand, OperationStepTarget, ProcessTool,
    RawWorkflowIr, SkillTarget, Source, StepAct, StepKind, ToolTarget, WorkerTool, Workflow,
    WorkflowStep, WorkflowWhen, validate_process_tool,
};
pub use install::{
    SkillInstallError, SkillInstallOrigin, ValidatedSkillInstall, validate_skill_install,
};
pub use manifest::{
    RUNX_VERSION, RawMethodManifestIr, SkillMethodManifest, parse_method_manifest_yaml,
    validate_method_manifest,
};
pub use package::{
    PROFILE_FILE, PackagePacket, SkillPackageError, SkillPackageSource, ValidatedJavaScriptModule,
    ValidatedPackageTool, ValidatedSkillPackage, javascript_module_imports, profile_directory,
    resolve_javascript_module_import, validate_skill_package,
};
pub use packet::{
    PACKET_ID_FIELD, PacketSchemaError, ValidatedPacketSchema, parse_packet_schema_document,
};
pub use registry_ref::{REGISTRY_REF_PREFIX, registry_ref};
pub use skill::{
    ArtifactPageFraming, ArtifactPageInput, ArtifactPageSource, CatalogMetadata, CatalogVisibility,
    ConnectorRequirement, InputMode, RawSkillIr, SkillArtifactContract, SkillIdempotencyPolicy,
    SkillInput, SkillMethodDefinition, ValidateSkillMode, ValidateSkillOptions, ValidatedSkill,
    agent_task_manual, agent_task_sections, constrained_input_schema, parse_skill_markdown,
    validate_input_examples, validate_skill, validate_skill_artifact_contract,
    validate_skill_with_options,
};
pub use test_fixture::{
    ReceiptExpectation, TestExpectation, TestFixture, TestHost, TestHttpExchangeFixture,
    TestHttpRequestBodyFixture, TestHttpResponseFixture, TestMcpCallFixture,
    TestProcessCallFixture, TestProviderAccess, TestProviderGrantFixture, TestProviderLeaseFixture,
    TestProviderOperationFixture, TestProviderResponsesFixture, TestProviders, parse_test_fixture,
};
pub use tool::{
    RawToolManifestIr, ValidatedTool, parse_tool_manifest_json, parse_tool_manifest_yaml,
    validate_tool_manifest,
};
pub use yaml::{
    assert_execution_profile_yaml_subset, assert_yaml_supported_subset, parse_yaml_document,
};
