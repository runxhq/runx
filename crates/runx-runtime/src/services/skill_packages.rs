mod change;
mod path;
mod snapshot;
mod staging;
mod validation;
mod workspace;

pub(crate) use change::{
    apply_skill_change, bind_skill_change, plan_skill_architecture, skill_change_effect,
};
pub(crate) use validation::validate_skill_package;
pub(crate) use workspace::inspect_skill_workspace;

use crate::skill_package::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

#[cfg(test)]
use path::{assert_allowed_package_delete_path, assert_allowed_package_write_path};
#[cfg(test)]
use snapshot::package_snapshot;
#[cfg(test)]
use staging::CandidateStage;
#[cfg(test)]
mod tests;
