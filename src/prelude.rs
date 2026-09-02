//! Ambient JavaScript library declarations and target-environment selection.
//!
//! User-authored `/*#rt */` annotations remain separate from this catalog. The
//! legacy `merge_prelude` adapter is retained while the verifier migrates to
//! type-directed global, member, and module lookup.

mod catalog;
mod environment;
mod legacy;
mod model;

pub use environment::{
    Environment, EnvironmentError, EnvironmentEvidence, EvidenceKind, ParseEnvironmentError,
    detect_environment, resolve_environment,
};
pub use model::{
    CallbackTiming, CallbackUse, FunctionEffects, FunctionSignature, LibraryExport, LibraryModule,
    LibraryParameter, LibraryRegistry, PropertySignature, ReceiverEffect, SemanticRefinement,
};

use crate::syntax::Annotation;

/// Build the standard-library catalog for an explicitly resolved environment.
/// Use [`registry_for_source`] when the requested environment is `Auto`.
pub fn registry(environment: Environment) -> Result<LibraryRegistry, EnvironmentError> {
    if environment == Environment::Auto {
        return Err(EnvironmentError::AutoRequiresSource);
    }
    Ok(catalog::build(environment))
}

/// Resolve `Auto` from the source and build its deterministic library catalog.
pub fn registry_for_source(
    requested: Environment,
    source: &str,
) -> Result<LibraryRegistry, EnvironmentError> {
    let environment = resolve_environment(requested, source)?;
    Ok(catalog::build(environment))
}

/// Compatibility adapter for the original verifier entrypoint.
///
/// It intentionally exposes only the four signatures understood by the
/// string-keyed verifier. New integrations should query [`LibraryRegistry`].
pub fn merge_prelude(annotations: &mut Vec<Annotation>) {
    legacy::append(annotations);
}

#[cfg(test)]
mod tests;
