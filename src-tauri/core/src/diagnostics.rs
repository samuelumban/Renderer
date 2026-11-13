//! User-facing errors: plain summary, command, exit code, excerpt, likely
//! cause (flagged as hypothesis when uncertain) and a safe suggestion.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Diagnostic {
    pub summary: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub excerpt: Option<String>,
    pub likely_cause: Option<String>,
    pub cause_is_hypothesis: bool,
    pub suggestion: Option<String>,
}

impl Diagnostic {
    pub fn new(summary: impl Into<String>) -> Self {
        Diagnostic { summary: summary.into(), ..Default::default() }
    }
    pub fn suggest(mut self, s: impl Into<String>) -> Self {
        self.suggestion = Some(s.into());
        self
    }
    pub fn cause(mut self, s: impl Into<String>, hypothesis: bool) -> Self {
        self.likely_cause = Some(s.into());
        self.cause_is_hypothesis = hypothesis;
        self
    }
    pub fn io(context: &str, e: std::io::Error) -> Self {
        Diagnostic::new(format!("{context}: {e}"))
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary)
    }
}

impl From<std::io::Error> for Diagnostic {
    fn from(e: std::io::Error) -> Self {
        Diagnostic::new(format!("File system error: {e}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Install,
    Preview,
    Compositions,
    Render,
}

struct Rule {
    needles: &'static [&'static str],
    cause: &'static str,
    certain: bool,
    suggestion: &'static str,
}

const COMMON: &[Rule] = &[
    Rule {
        needles: &["ENOSPC", "no space left on device", "There is not enough space"],
        cause: "The disk is full.",
        certain: true,
        suggestion: "Free disk space (Storage > Clean temporary files / Clear package cache) and retry.",
    },
    Rule {
        needles: &["ETIMEDOUT", "ENOTFOUND", "ECONNRESET", "EAI_AGAIN", "ECONNREFUSED", "network request", "socket hang up"],
        cause: "A network request failed.",
        certain: false,
        suggestion: "Check the internet connection, proxy or firewall, then retry.",
    },
    Rule {
        needles: &["EPERM", "EACCES", "EBUSY"],
        cause: "A file was locked or access was denied (antivirus scanners and open editors commonly cause this).",
        certain: false,
        suggestion: "Close programs using the project folder and retry. Do not run the app as Administrator.",
    },
    Rule {
        needles: &["JavaScript heap out of memory", "ENOMEM", "Cannot allocate memory"],
        cause: "The process ran out of memory.",
        certain: false,
        suggestion: "Lower the render concurrency, close other applications and retry.",
    },
];

const INSTALL: &[Rule] = &[
    Rule {
        needles: &["can only install packages when your package.json and package-lock.json", "ERR_PNPM_OUTDATED_LOCKFILE", "YN0028", "Your lockfile needs to be updated", "npm ci` can only install"],
        cause: "The lockfile does not match package.json.",
        certain: true,
        suggestion: "Regenerate the lockfile in the source project and re-export the ZIP. This app never rewrites a lockfile on its own.",
    },
    Rule {
        needles: &["ERESOLVE"],
        cause: "The project's dependencies have conflicting peer requirements.",
        certain: true,
        suggestion: "Fix the dependency versions in the source project. This app does not change package versions automatically.",
    },
    Rule {
        needles: &["EBADENGINE", "Unsupported engine", "is incompatible with this module", "ERR_PNPM_UNSUPPORTED_ENGINE"],
        cause: "A dependency does not support the selected Node.js version.",
        certain: false,
        suggestion: "Set `engines.node` in the project's package.json to a supported version and re-import.",
    },
    Rule {
        needles: &["node-gyp", "gyp ERR"],
        cause: "A native dependency needs C/C++ build tools that are not available.",
        certain: false,
        suggestion: "This project has native dependencies. Install the Visual Studio Build Tools, or use a prebuilt version of the dependency.",
    },
    Rule {
        needles: &["ELIFECYCLE", "postinstall", "preinstall"],
        cause: "A package lifecycle script failed.",
        certain: false,
        suggestion: "Inspect the log. If the script is optional, retry with Safe install (lifecycle scripts disabled).",
    },
];

const RENDER: &[Rule] = &[
    Rule {
        needles: &["remotion-ffmpeg-binaries"],
        cause: "This older Remotion version downloads FFmpeg from a server that no longer provides it.",
        certain: true,
        suggestion: "Install FFmpeg and make it available on PATH, or upgrade the source project to Remotion 4 (which bundles FFmpeg).",
    },
    Rule {
        needles: &["Could not find composition", "No composition with the ID", "composition with the id"],
        cause: "The composition ID does not exist in this project.",
        certain: true,
        suggestion: "Use Detect compositions and pick one of the listed IDs.",
    },
    Rule {
        needles: &["Failed to launch the browser", "Could not find a browser", "browser process", "chrome-headless-shell", "Chrome Headless Shell", "ERR_PUPPETEER"],
        cause: "Remotion's rendering browser could not be downloaded or started.",
        certain: false,
        suggestion: "Check the internet connection (the browser downloads on first use) or set a browser executable in Settings.",
    },
    Rule {
        needles: &["delayRender", "A delayRender()"],
        cause: "An asset (font, image, video or data) did not finish loading in time.",
        certain: false,
        suggestion: "Check the Fonts tab and any network assets used by the composition, then retry.",
    },
    Rule {
        needles: &["Cannot find module", "Module not found", "Can't resolve"],
        cause: "A module is missing from the installed dependencies or the project source.",
        certain: false,
        suggestion: "Use Rebuild environment. If it still fails, the dependency is missing from the project's package.json.",
    },
    Rule {
        needles: &["EADDRINUSE"],
        cause: "The local port is already in use.",
        certain: true,
        suggestion: "Retry; a new free port is chosen each time.",
    },
    Rule {
        needles: &["ffmpeg", "FFmpeg", "Unknown encoder", "codec"],
        cause: "Video encoding failed.",
        certain: false,
        suggestion: "Try the h264 codec with an .mp4 container, and check that the output folder is writable.",
    },
    Rule {
        needles: &["Unknown argument", "Unknown option", "unknown flag"],
        cause: "This Remotion version does not support one of the selected render options.",
        certain: false,
        suggestion: "Reset scale / frame range / codec to their defaults and retry.",
    },
];

/// Build a diagnostic for a failed child process from the tail of its output.
pub fn diagnose(stage: Stage, command: &str, exit_code: Option<i32>, tail: &str) -> Diagnostic {
    let what = match stage {
        Stage::Install => "Dependency installation failed",
        Stage::Preview => "Remotion Studio stopped unexpectedly",
        Stage::Compositions => "Could not list compositions",
        Stage::Render => "Render failed",
    };
    let mut d = Diagnostic::new(match exit_code {
        Some(c) => format!("{what} (exit code {c})."),
        None => format!("{what}."),
    });
    d.command = Some(command.to_string());
    d.exit_code = exit_code;
    d.excerpt = Some(excerpt(tail, 25));
    let specific: &[Rule] = if stage == Stage::Install { INSTALL } else { RENDER };
    let lower = tail.to_lowercase();
    for rule in specific.iter().chain(COMMON) {
        if rule.needles.iter().any(|n| lower.contains(&n.to_lowercase())) {
            d.likely_cause = Some(rule.cause.to_string());
            d.cause_is_hypothesis = !rule.certain;
            d.suggestion = Some(rule.suggestion.to_string());
            return d;
        }
    }
    d.likely_cause = Some("Unknown. See the log excerpt.".into());
    d.cause_is_hypothesis = true;
    d.suggestion = Some("Open the Logs tab for the full output, or use Copy diagnostics when asking for help.".into());
    d
}

/// Last `max_lines` non-empty lines, preferring lines that look like errors.
pub fn excerpt(tail: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfile_mismatch_is_certain() {
        let d = diagnose(Stage::Install, "npm ci", Some(1), "npm error `npm ci` can only install packages when your package.json and package-lock.json are in sync");
        assert!(!d.cause_is_hypothesis);
        assert!(d.likely_cause.unwrap().contains("lockfile"));
    }

    #[test]
    fn unknown_failure_is_hypothesis() {
        let d = diagnose(Stage::Render, "node cli render", Some(1), "something odd");
        assert!(d.cause_is_hypothesis);
        assert_eq!(d.exit_code, Some(1));
    }
}
