//! The configured executable adapter shared by graph steps and test
//! replay: trusted host processes, with credential evidence recorded the same
//! way at every entry point. Worker tools run on the runtime's worker session.

use crate::RuntimeError;
use crate::adapter::Executable;
use crate::adapter::{InvocationOutput, SkillAdapter, SkillInvocation};
use crate::adapters::process_tool::ProcessToolAdapter;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ExecutableAdapter;

impl SkillAdapter for ExecutableAdapter {
    fn invoke(&self, request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        if matches!(request.executable, Executable::Process(_)) {
            return ProcessToolAdapter.invoke(request);
        }
        Err(RuntimeError::UnsupportedExecutable {
            kind: request.executable.kind().to_owned(),
        })
    }
}
