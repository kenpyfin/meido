//! Meido-prefixed environment keys, with `FINALLY_A_VALUE_BOT_*` aliases for compatibility.

/// Preferred / legacy names for one Meido meta env key.
pub struct MeidoEnvKey {
    pub preferred: &'static str,
    pub legacy: &'static str,
}

pub const CONFIG: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_CONFIG",
    legacy: "FINALLY_A_VALUE_BOT_CONFIG",
};

pub const SELF_REPO: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_SELF_REPO",
    legacy: "FINALLY_A_VALUE_BOT_SELF_REPO",
};

pub const BUILTIN_SKILLS: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_BUILTIN_SKILLS",
    legacy: "FINALLY_A_VALUE_BOT_BUILTIN_SKILLS",
};

pub const BUILTIN_HOOKS: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_BUILTIN_HOOKS",
    legacy: "FINALLY_A_VALUE_BOT_BUILTIN_HOOKS",
};

pub const IN_DOCKER: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_IN_DOCKER",
    legacy: "FINALLY_A_VALUE_BOT_IN_DOCKER",
};

pub const GATEWAY: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_GATEWAY",
    legacy: "FINALLY_A_VALUE_BOT_GATEWAY",
};

pub const RUNTIME_DATA: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_RUNTIME_DATA",
    legacy: "FINALLY_A_VALUE_BOT_RUNTIME_DATA",
};

pub const REPO: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_REPO",
    legacy: "FINALLY_A_VALUE_BOT_REPO",
};

pub const INSTALL_DIR: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_INSTALL_DIR",
    legacy: "FINALLY_A_VALUE_BOT_INSTALL_DIR",
};

pub const REPO_ROOT: MeidoEnvKey = MeidoEnvKey {
    preferred: "MEIDO_REPO_ROOT",
    legacy: "FINALLY_A_VALUE_BOT_REPO_ROOT",
};

/// Non-secret Meido meta keys (preferred + legacy) for redaction skip lists.
pub const NON_SECRET_KEYS: &[&str] = &[
    CONFIG.preferred,
    CONFIG.legacy,
    BUILTIN_SKILLS.preferred,
    BUILTIN_SKILLS.legacy,
    BUILTIN_HOOKS.preferred,
    BUILTIN_HOOKS.legacy,
    SELF_REPO.preferred,
    SELF_REPO.legacy,
    RUNTIME_DATA.preferred,
    RUNTIME_DATA.legacy,
];

fn trim_nonempty(raw: String) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Read preferred key first, then legacy alias.
pub fn get(key: MeidoEnvKey) -> Option<String> {
    std::env::var(key.preferred)
        .ok()
        .and_then(trim_nonempty)
        .or_else(|| std::env::var(key.legacy).ok().and_then(trim_nonempty))
}

/// True when either preferred or legacy is set (including empty, for marker flags).
pub fn is_present(key: MeidoEnvKey) -> bool {
    std::env::var_os(key.preferred).is_some() || std::env::var_os(key.legacy).is_some()
}

/// True when preferred or legacy equals `"1"`.
pub fn is_flag_one(key: MeidoEnvKey) -> bool {
    matches!(get(key).as_deref(), Some("1"))
}
