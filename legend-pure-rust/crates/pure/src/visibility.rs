// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Repo-boundary visibility check (Java-parity with `VisibilityValidation`).
//!
//! A repo can reference an element only if the element's home repo is the
//! repo itself or one of its directly-declared dependencies. Java's
//! [`Visibility.isVisible`][1] is one-hop: `repo == other ||
//! dependencies.contains(other.name)`. The transitive walk in
//! `getRepositoryDependenciesByName` is for `RepositoryPackageValidator`,
//! a different consumer.
//!
//! Sources whose path doesn't carry a `/{repo}/...` segment (the M3
//! bootstrap, hand-built test fixtures) have no enforceable repo and are
//! always visible. This matches Java returning `true` when
//! `getSourceRepoName` is `null` or the repo lookup fails.
//!
//! [1]: ../../legend-pure-core/legend-pure-m3-core/src/main/java/org/finos/legend/pure/m3/compiler/visibility/Visibility.java

use std::collections::{BTreeSet, HashMap};

use smol_str::SmolStr;

use crate::ids::ElementId;
use crate::model::PureModel;

/// Per-repo visibility map: repo name → its visible-set
/// (its direct dependencies plus itself).
///
/// Stored on [`PureModel::repo_visibility`]. Empty by default — a model
/// constructed without going through [`crate::pipeline::init_bootstrap_model`]
/// + a real loader sees no rules, so no checks run. This preserves every
///   existing test that builds a model from raw source files without a
///   surrounding repo.
pub type RepoVisibilityMap = HashMap<SmolStr, BTreeSet<SmolStr>>;

/// A repo's `pattern` regex paired with the original source string.
///
/// The original `source` is preserved verbatim so error messages can
/// quote it back to the user (Java parity:
/// `RepositoryPackageValidator` reports
/// `"only packages matching <pattern> are allowed"`).
///
/// `compiled` is the result of [`compile_repo_pattern`]: the source
/// wrapped in `^(?:…)$` so Rust's [`regex::Regex::is_match`] (unanchored
/// by default) matches Java's `java.util.regex.Matcher::matches`
/// (fully anchored).
#[derive(Debug, Clone)]
pub struct RepoPattern {
    /// Java-syntax pattern as written in the descriptor (e.g.
    /// `((meta)|(system)|(apps::pure))(::.*)?`).
    pub source: SmolStr,
    /// Compiled, anchored regex matching the same language as
    /// Java's `Pattern.compile(source).matcher(s).matches()`.
    pub compiled: regex::Regex,
}

/// Per-repo pattern table: repo name → its compiled allowed-package
/// pattern. Stored on [`PureModel::repo_patterns`][m]. Empty by default
/// — when no patterns are registered, the membership check is a no-op,
/// preserving every existing test that builds a model from raw source
/// files without going through a real loader.
///
/// [m]: crate::model::PureModel::repo_patterns
pub type RepoPatternMap = HashMap<SmolStr, RepoPattern>;

/// Compile a Java-syntax repo pattern into an anchored Rust [`regex::Regex`].
///
/// Wraps the input as `^(?:src)$` so [`regex::Regex::is_match`]
/// (unanchored by default in Rust) matches the same language as Java's
/// `Pattern.compile(src).matcher(s).matches()` (fully anchored). The
/// repo descriptors today use only basic alternation + grouping
/// (e.g. `((meta)|(system)|(apps::pure))(::.*)?`) — fully compatible
/// between Java's and Rust's regex flavours.
///
/// # Errors
///
/// Returns the underlying [`regex::Error`] if `src` is not a valid Rust
/// regex. Callers (today: the snapshot-builder descriptor loader) should
/// surface this as a build-time error so a malformed descriptor stops
/// the build immediately.
///
/// ```
/// use legend_pure_parser_pure::visibility::compile_repo_pattern;
/// let re = compile_repo_pattern("(meta)(::.*)?").expect("valid");
/// assert!(re.is_match("meta"));
/// assert!(re.is_match("meta::pure"));
/// // Anchored: `metadata` must NOT match `(meta)(::.*)?`.
/// assert!(!re.is_match("metadata"));
/// ```
pub fn compile_repo_pattern(src: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&format!("^(?:{src})$"))
}

/// Extract the repo name from a canonical source URL.
///
/// Mirrors Java's `CompositeCodeStorage.getSourceRepoName`: the first
/// path segment after a leading `/`, returning `None` when the path is
/// empty, doesn't start with `/`, or has no second segment.
///
/// ```
/// use legend_pure_parser_pure::visibility::source_repo_name;
/// assert_eq!(source_repo_name("/system/foo.pure").as_deref(), Some("system"));
/// assert_eq!(source_repo_name("/platform/pure/grammar/m3.pure").as_deref(), Some("platform"));
/// assert_eq!(source_repo_name("system/foo.pure"), None);
/// assert_eq!(source_repo_name("/onlyone"), None);
/// assert_eq!(source_repo_name(""), None);
/// ```
#[must_use]
pub fn source_repo_name(source: &str) -> Option<SmolStr> {
    if !source.starts_with('/') || source.len() < 2 {
        return None;
    }
    let rest = &source[1..];
    let end = rest.find('/')?;
    if end == 0 {
        return None;
    }
    Some(SmolStr::new(&rest[..end]))
}

/// Outcome of a visibility check. The error variant carries everything
/// the caller needs to format the Java-shaped diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibilityViolation {
    /// Fully-qualified path of the target element (e.g.
    /// `"datamarts::datamt::domain::TestClass2"`).
    pub target_fqn: SmolStr,
    /// Source path of the use site (e.g. `"/system/testFile.pure"`).
    pub use_site_source: SmolStr,
}

impl VisibilityViolation {
    /// Java-parity message: `"<fqn> is not visible in the file <source>"`.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "{fqn} is not visible in the file {src}",
            fqn = self.target_fqn,
            src = self.use_site_source
        )
    }
}

/// Check whether `target_id` is visible from a use site whose source
/// path is `use_site_source`.
///
/// Returns `Some(violation)` only when both the use-site repo and the
/// target's home repo can be determined *and* the use-site repo's
/// visible set does not include the target's repo. Returns `None` (i.e.
/// passes) in every other case:
///
/// - No visibility map populated (legacy / unit-test models).
/// - Use site has no extractable repo name (synthetic source).
/// - Use site repo is not registered in the map (test fixture loaded
///   without `from_descriptor`).
/// - Target is a [`crate::ids::ElementId::Package`] — packages are
///   visible globally; Java's package-allowed walk is a finer-grained
///   check we deliberately skip in v1.
/// - Target's source path has no extractable repo name (bootstrap
///   primitives, M3 metaclasses).
/// - Target's home repo is in the use-site repo's visible set.
#[must_use]
pub fn check_element_visible(
    model: &PureModel,
    use_site_source: &SmolStr,
    target_id: ElementId,
) -> Option<VisibilityViolation> {
    // No visibility rules registered → nothing to enforce.
    if model.repo_visibility.is_empty() {
        return None;
    }

    let use_repo = source_repo_name(use_site_source)?;

    // Use-site repo not in the map (e.g. test fixture loaded without a
    // descriptor) → no rules to enforce.
    let visible = model.repo_visibility.get(&use_repo)?;

    // Packages are visible globally in v1. Java does a per-repo
    // `isPackageAllowed` walk; not yet replicated.
    if matches!(target_id, ElementId::Package(_)) {
        return None;
    }

    let target_node = model.try_get_element(target_id).and_then(|_| {
        if matches!(target_id, ElementId::InstanceId { .. }) {
            Some(model.get_node(target_id))
        } else {
            None
        }
    })?;
    let target_source = &target_node.source_info.source;
    let Some(target_repo) = source_repo_name(target_source) else {
        // No extractable repo on the target (bootstrap / M3) → visible.
        return None;
    };

    if visible.contains(&target_repo) {
        return None;
    }

    Some(VisibilityViolation {
        target_fqn: SmolStr::new(
            crate::purem::fqn_path::element_fqn_path(model, target_id).join("::"),
        ),
        use_site_source: use_site_source.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_repo_name_basic() {
        assert_eq!(
            source_repo_name("/platform/pure/grammar/m3.pure").as_deref(),
            Some("platform")
        );
        assert_eq!(
            source_repo_name("/system/testFile.pure").as_deref(),
            Some("system")
        );
        assert_eq!(
            source_repo_name("/datamart_datamt/x.pure").as_deref(),
            Some("datamart_datamt")
        );
    }

    #[test]
    fn extract_repo_name_rejects_no_leading_slash() {
        assert_eq!(source_repo_name("system/foo.pure"), None);
        assert_eq!(source_repo_name("foo.pure"), None);
    }

    #[test]
    fn extract_repo_name_rejects_no_second_segment() {
        assert_eq!(source_repo_name("/"), None);
        assert_eq!(source_repo_name("/onlyone"), None);
        assert_eq!(source_repo_name(""), None);
    }

    #[test]
    fn extract_repo_name_rejects_empty_first_segment() {
        // "//foo" → first segment is empty.
        assert_eq!(source_repo_name("//foo"), None);
    }
}
