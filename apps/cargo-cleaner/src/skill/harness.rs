use clap::builder::{PossibleValue, PossibleValuesParser};
use std::{
    collections::BTreeSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const SHARED: &str = ".agents/skills";
const XDG_AGENTS: &str = ".config/agents/skills";

struct Harness(&'static [&'static str], &'static str, &'static str);

// Project and user defaults follow the GitHub CLI and Vercel skills registries.
// https://github.com/cli/cli/blob/trunk/internal/skills/registry/registry.go
// https://github.com/vercel-labs/skills/blob/main/docs/agents.md
const HARNESSES: &[Harness] = &[
    Harness(&["codex"], SHARED, SHARED),
    Harness(
        &["claude", "claude-code"],
        ".claude/skills",
        ".claude/skills",
    ),
    Harness(&["copilot", "github-copilot"], SHARED, ".copilot/skills"),
    Harness(&["cursor", "cursor-agent"], SHARED, ".cursor/skills"),
    Harness(&["gemini", "gemini-cli"], SHARED, ".gemini/skills"),
    Harness(&["opencode"], SHARED, ".config/opencode/skills"),
    Harness(&["pi"], ".pi/skills", ".pi/agent/skills"),
    Harness(
        &["deepseek", "dsh", "deepseek-harness"],
        ".dsh/skills",
        ".dsh/skills",
    ),
    Harness(&["agents", "universal"], SHARED, SHARED),
    Harness(&["adal"], ".adal/skills", ".adal/skills"),
    Harness(&["aider-desk"], ".aider-desk/skills", ".aider-desk/skills"),
    Harness(&["amp"], SHARED, XDG_AGENTS),
    Harness(&["antigravity"], SHARED, ".gemini/antigravity/skills"),
    Harness(
        &["antigravity-cli", "agy"],
        SHARED,
        ".gemini/antigravity-cli/skills",
    ),
    Harness(&["antigravity2.0"], SHARED, ".gemini/config/skills"),
    Harness(&["augment", "auggie"], ".augment/skills", ".augment/skills"),
    Harness(
        &["autohand-code", "autohand"],
        ".autohand/skills",
        ".autohand/skills",
    ),
    Harness(&["bob"], ".bob/skills", ".bob/skills"),
    Harness(&["cline"], SHARED, SHARED),
    Harness(&["codebuddy"], ".codebuddy/skills", ".codebuddy/skills"),
    Harness(
        &["command-code"],
        ".commandcode/skills",
        ".commandcode/skills",
    ),
    Harness(&["continue", "cn"], ".continue/skills", ".continue/skills"),
    Harness(&["cortex"], ".cortex/skills", ".snowflake/cortex/skills"),
    Harness(&["crush"], ".crush/skills", ".config/crush/skills"),
    Harness(&["deepagents"], SHARED, ".deepagents/agent/skills"),
    Harness(&["devin"], ".devin/skills", ".config/devin/skills"),
    Harness(&["droid", "factory"], SHARED, ".factory/skills"),
    Harness(&["firebender"], SHARED, ".firebender/skills"),
    Harness(&["goose"], ".goose/skills", ".config/goose/skills"),
    Harness(&["grok"], ".grok/skills", ".grok/skills"),
    Harness(
        &["hermes", "hermes-agent"],
        ".hermes/skills",
        ".hermes/skills",
    ),
    Harness(&["iflow", "iflow-cli"], ".iflow/skills", ".iflow/skills"),
    Harness(&["junie"], ".junie/skills", ".junie/skills"),
    Harness(&["kilo", "kilocode"], ".kilocode/skills", ".kilo/skills"),
    Harness(&["kimi", "kimi-cli", "kimi-code-cli"], SHARED, XDG_AGENTS),
    Harness(&["kiro", "kiro-cli"], ".kiro/skills", ".kiro/skills"),
    Harness(&["kode"], ".kode/skills", ".kode/skills"),
    Harness(&["mcpjam"], ".mcpjam/skills", ".mcpjam/skills"),
    Harness(&["mistral-vibe", "vibe"], ".vibe/skills", ".vibe/skills"),
    Harness(&["mux"], ".mux/skills", ".mux/skills"),
    Harness(&["neovate"], ".neovate/skills", ".neovate/skills"),
    Harness(&["openclaw"], "skills", ".openclaw/skills"),
    Harness(&["openhands"], ".openhands/skills", ".openhands/skills"),
    Harness(&["pochi"], ".pochi/skills", ".pochi/skills"),
    Harness(&["qoder", "qodercli"], ".qoder/skills", ".qoder/skills"),
    Harness(&["qwen", "qwen-code"], ".qwen/skills", ".qwen/skills"),
    Harness(&["replit"], SHARED, XDG_AGENTS),
    Harness(&["roo", "roo-code"], ".roo/skills", ".roo/skills"),
    Harness(
        &["tabnine-cli"],
        ".tabnine/agent/skills",
        ".tabnine/agent/skills",
    ),
    Harness(&["trae"], ".trae/skills", ".trae/skills"),
    Harness(&["trae-cn"], ".trae/skills", ".trae-cn/skills"),
    Harness(&["warp"], SHARED, SHARED),
    Harness(
        &["windsurf"],
        ".windsurf/skills",
        ".codeium/windsurf/skills",
    ),
    Harness(&["zed"], SHARED, SHARED),
    Harness(&["zencoder"], ".zencoder/skills", ".zencoder/skills"),
];

pub fn parser() -> PossibleValuesParser {
    PossibleValuesParser::new(HARNESSES.iter().map(|Harness(names, _, _)| {
        PossibleValue::new(names[0]).aliases(names[1..].iter().copied())
    }))
}

pub fn directories(
    names: &[String],
    local: bool,
    detect: bool,
    root: &Path,
    home: Option<&Path>,
) -> BTreeSet<&'static Path> {
    let path = env::var_os("PATH").unwrap_or_default();
    let search: Vec<PathBuf> = env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .collect();
    let mut directories = BTreeSet::new();
    for &Harness(aliases, project, user) in HARNESSES {
        let selected = if !names.is_empty() {
            names.iter().any(|name| aliases.contains(&name.as_str()))
        } else if detect {
            let config = Path::new(user).parent().expect("skill paths have a parent");
            let project_config = Path::new(project)
                .parent()
                .expect("skill paths have a parent");
            // Shared directories and the conventional "skills" folder don't identify a harness.
            (user != SHARED
                && user != XDG_AGENTS
                && home.is_some_and(|home| home.join(config).is_dir()))
                || (local
                    && project != SHARED
                    && !project_config.as_os_str().is_empty()
                    && root.join(project_config).is_dir())
                || aliases.iter().any(|name| {
                    search.iter().any(|bin| {
                        fs::metadata(bin.join(name)).is_ok_and(|meta| {
                            meta.is_file() && meta.permissions().mode() & 0o111 != 0
                        })
                    })
                })
        } else {
            true
        };
        if selected {
            directories.insert(Path::new(if local { project } else { user }));
        }
    }
    if directories.is_empty() && detect {
        directories.insert(Path::new(SHARED));
    }
    directories
}
