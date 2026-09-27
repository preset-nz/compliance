//! The facts a scan records about one package. Verdicts are not facts: `check`
//! computes them from these rows plus the config and preset on every run.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Cargo,
    Pnpm,
    Uv,
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Ecosystem::Cargo => "cargo",
            Ecosystem::Pnpm => "pnpm",
            Ecosystem::Uv => "uv",
        })
    }
}

/// How a package reaches the repo's own code. Ordered weakest first, so the
/// strongest scope of several paths is `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Only reachable through dev-dependencies: tests, benches, examples.
    Dev,
    /// Runs at build time and is not linked into what ships: build scripts,
    /// proc-macros and everything they pull in.
    Build,
    /// Reachable through normal dependency edges from the repo's own code.
    Shipped,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Shipped, Scope::Build, Scope::Dev];
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Scope::Dev => "dev",
            Scope::Build => "build",
            Scope::Shipped => "shipped",
        })
    }
}

/// Where a package came from, without machine-specific paths. Stored in the
/// lock as a flat string: `registry`, `registry+<url>`, `git+<url>`, `path`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Source {
    /// The ecosystem's default public registry (crates.io, npmjs, PyPI).
    Registry,
    /// Another registry, recorded by URL.
    RegistryUrl(String),
    /// A git repository, recorded by URL and revision.
    Git(String),
    /// A local path outside the repo's own workspace.
    Path,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Registry => f.write_str("registry"),
            Source::RegistryUrl(url) => write!(f, "registry+{url}"),
            Source::Git(url) => write!(f, "git+{url}"),
            Source::Path => f.write_str("path"),
        }
    }
}

impl From<Source> for String {
    fn from(source: Source) -> String {
        source.to_string()
    }
}

impl TryFrom<String> for Source {
    type Error = String;

    fn try_from(raw: String) -> Result<Source, String> {
        match raw.as_str() {
            "registry" => Ok(Source::Registry),
            "path" => Ok(Source::Path),
            _ => {
                if let Some(url) = raw.strip_prefix("registry+") {
                    Ok(Source::RegistryUrl(url.to_string()))
                } else if let Some(url) = raw.strip_prefix("git+") {
                    Ok(Source::Git(url.to_string()))
                } else {
                    Err(format!("unknown source `{raw}`"))
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Package {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
    pub source: Source,
    /// The licence exactly as the package declares it. `None` when it
    /// declares nothing; `check` then needs a `clarify` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<String>,
    pub scope: Scope,
}
