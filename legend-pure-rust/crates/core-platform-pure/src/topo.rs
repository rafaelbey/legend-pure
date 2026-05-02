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

//! Topological sort of [`Repo`]s by their declared `dependencies`.
//!
//! [`crate::repo::load`] processes repos in dependency order so cross-repo
//! references in a `.purem` slice always resolve against an
//! already-loaded ancestor repo. Each [`crate::repo::RepoMeta`] carries
//! its `dependencies: &[&str]` list, parsed from the Java descriptor
//! JSON; this module turns that adjacency list into a load-ready
//! ordering.

use thiserror::Error;

use crate::repo::Repo;

/// Errors raised by [`topo_sort_repos`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TopoError {
    /// A repo declared a dependency on a name not present in the repo
    /// list. Almost always a typo or missing classpath entry.
    #[error("repo '{from}' depends on unknown repo '{missing}'")]
    MissingDep {
        /// Name of the repo declaring the missing dep.
        from: String,
        /// Name of the missing dependency.
        missing: String,
    },
    /// Cyclic dependency among repos. Includes the names that participate
    /// in the cycle for diagnostics.
    #[error("cyclic repo dependency among: {0:?}")]
    Cycle(Vec<String>),
    /// A repo carries no `RepoMeta`. Anonymous filesystem repos (built
    /// via `Repo::from_filesystem` without a descriptor) cannot be
    /// topo-sorted because they have no name. The caller should either
    /// rebuild them via `from_descriptor` or pass them in a list that
    /// has no other dependencies.
    #[error("repo at index {0} has no RepoMeta — cannot topo-sort anonymous repos")]
    AnonymousRepo(usize),
}

/// Topologically sort repos by their declared dependencies.
///
/// Returns repos in the order they should be loaded:
/// - `repos[i]`'s dependencies all appear before it in the output.
/// - For repos with no dependency relationship, ties broken by
///   stable insertion order (matches the input order).
///
/// # Errors
///
/// Returns [`TopoError::MissingDep`] for unresolvable dependency names,
/// [`TopoError::Cycle`] for circular dependencies, or
/// [`TopoError::AnonymousRepo`] for repos missing `RepoMeta`.
pub fn topo_sort_repos(repos: &[Repo]) -> Result<Vec<&Repo>, TopoError> {
    // Validate: every repo must have meta (we need its name).
    for (i, r) in repos.iter().enumerate() {
        if r.meta().is_none() {
            return Err(TopoError::AnonymousRepo(i));
        }
    }

    // Build name → index map.
    let mut name_to_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(repos.len());
    for (i, r) in repos.iter().enumerate() {
        let meta = r.meta().expect("checked above");
        name_to_idx.insert(meta.name, i);
    }

    // Validate: every declared dep is present.
    for r in repos {
        let meta = r.meta().expect("checked above");
        for dep in meta.dependencies {
            if !name_to_idx.contains_key(*dep) {
                return Err(TopoError::MissingDep {
                    from: meta.name.to_string(),
                    missing: (*dep).to_string(),
                });
            }
        }
    }

    // Kahn's algorithm with stable tie-breaking (ascending input index).
    let n = repos.len();
    let mut in_degree: Vec<usize> = vec![0; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, r) in repos.iter().enumerate() {
        let meta = r.meta().expect("checked above");
        for dep_name in meta.dependencies {
            let dep_idx = name_to_idx[*dep_name];
            adj[dep_idx].push(i);
            in_degree[i] += 1;
        }
    }

    // Initial frontier: zero in-degree, in input order.
    let mut frontier: std::collections::BTreeSet<usize> = (0..n)
        .filter(|&i| in_degree[i] == 0)
        .collect();

    let mut out: Vec<&Repo> = Vec::with_capacity(n);
    while let Some(&i) = frontier.iter().next() {
        frontier.remove(&i);
        out.push(&repos[i]);
        for &succ in &adj[i] {
            in_degree[succ] -= 1;
            if in_degree[succ] == 0 {
                frontier.insert(succ);
            }
        }
    }

    if out.len() != n {
        // Cycle. Collect every repo with non-zero in-degree.
        let mut in_cycle: Vec<String> = (0..n)
            .filter(|&i| in_degree[i] > 0)
            .map(|i| repos[i].meta().expect("checked above").name.to_string())
            .collect();
        in_cycle.sort();
        return Err(TopoError::Cycle(in_cycle));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{Repo, RepoMeta};

    fn make_meta(name: &'static str, deps: &'static [&'static str]) -> RepoMeta {
        RepoMeta {
            name,
            pattern: ".*",
            dependencies: deps,
        }
    }

    fn purem_repo(meta: RepoMeta) -> Repo {
        // Empty blob is fine for topo tests — we never read it.
        Repo::from_purem_bytes(
            format!("/{}", meta.name),
            meta,
            std::sync::Arc::from(vec![].into_boxed_slice()),
        )
    }

    // Pre-built static dep tables used across tests; `RepoMeta.dependencies`
    // requires `&'static [&'static str]` so we declare them as items.
    const NO_DEPS: &[&str] = &[];
    const DEPS_PLATFORM: &[&str] = &["platform"];
    const DEPS_STORE_AND_DIAGRAM: &[&str] = &["platform_dsl_store", "platform_dsl_diagram"];
    const DEPS_B: &[&str] = &["b"];
    const DEPS_A: &[&str] = &["a"];
    const DEPS_NONEXISTENT: &[&str] = &["nonexistent"];

    #[test]
    fn topo_orders_diamond() {
        let platform_meta = make_meta("platform", NO_DEPS);
        let store_meta = make_meta("platform_dsl_store", DEPS_PLATFORM);
        let diagram_meta = make_meta("platform_dsl_diagram", DEPS_PLATFORM);
        let mapping_meta = make_meta("platform_dsl_mapping", DEPS_STORE_AND_DIAGRAM);

        // Insert in shuffled order to confirm the sort really runs.
        let repos = vec![
            purem_repo(mapping_meta),
            purem_repo(diagram_meta),
            purem_repo(store_meta),
            purem_repo(platform_meta),
        ];

        let sorted = topo_sort_repos(&repos).expect("sort");
        let names: Vec<&str> = sorted.iter().map(|r| r.meta().unwrap().name).collect();
        assert_eq!(names[0], "platform");
        // The two siblings appear in a deterministic order (input-stable).
        assert!(names[1] == "platform_dsl_store" || names[1] == "platform_dsl_diagram");
        assert!(names[2] == "platform_dsl_store" || names[2] == "platform_dsl_diagram");
        assert_eq!(names[3], "platform_dsl_mapping");
    }

    #[test]
    fn topo_detects_cycle() {
        let repos = vec![
            purem_repo(make_meta("a", DEPS_B)),
            purem_repo(make_meta("b", DEPS_A)),
        ];
        match topo_sort_repos(&repos) {
            Err(TopoError::Cycle(names)) => {
                assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected Cycle, got {other:?}"),
        }
    }

    #[test]
    fn topo_detects_missing_dep() {
        let repos = vec![purem_repo(make_meta("a", DEPS_NONEXISTENT))];
        match topo_sort_repos(&repos) {
            Err(TopoError::MissingDep { from, missing }) => {
                assert_eq!(from, "a");
                assert_eq!(missing, "nonexistent");
            }
            other => panic!("expected MissingDep, got {other:?}"),
        }
    }
}
