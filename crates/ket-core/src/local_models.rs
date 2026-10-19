//! Local coding agents: OpenCode driving a model Ollama runs on this Mac.
//!
//! The data and arithmetic behind them, with no I/O of its own — what an
//! agent is called, which models are worth recommending, and whether one
//! fits this Mac's memory at a given context length. [`crate::ollama`] asks
//! the daemon, [`crate::machine`] the hardware, and
//! [`crate::agents::Catalogue::detect`] puts the answers together.
//!
//! A model becomes an agent only through its **profile**: a copy Ollama
//! makes of it pinned to a context length ([`profile_tag`]). Left to itself
//! Ollama loads a model at a context chosen from the machine's memory — 4k
//! on a 16 GB Mac, where an agent's tool calls fail, and the model's whole
//! maximum on a large one, where the KV cache can take more memory than the
//! weights. The profile fixes it at [`context_for`]'s answer, and costs no
//! disk: the copy shares the original's blobs.

use crate::ollama::{ModelInfo, Snapshot};

/// The prefix of a local agent's name: `opencode:ollama/qwen3-coder:30b`.
/// The part before the first `:` is the harness — see
/// [`crate::agents::harness`].
pub const AGENT_PREFIX: &str = "opencode:ollama/";

/// The shortest context ket offers an agent at. Below it a coding agent's
/// tool calls and file contents do not fit.
pub const MIN_CONTEXT: u32 = 16_384;

/// The context lengths ket asks for, longest first.
const CONTEXTS: [u32; 3] = [65_536, 32_768, MIN_CONTEXT];

/// KV bytes per token when the model's shape is not known: about what a
/// 30B-class model with grouped-query attention takes.
pub const DEFAULT_KV_PER_TOKEN: u64 = 128 * 1024;

/// The local agent `tag` runs as.
pub fn agent_name(tag: &str) -> String {
    format!("{AGENT_PREFIX}{tag}")
}

/// The model a local agent's name runs, or `None` for any other agent.
pub fn model_tag(agent: &str) -> Option<&str> {
    agent
        .strip_prefix(AGENT_PREFIX)
        .filter(|tag| !tag.is_empty())
}

/// The name of `tag`'s agent profile: the same name with `-ket` after it.
pub fn profile_tag(tag: &str) -> String {
    format!("{tag}-ket")
}

/// What a local agent's OpenCode is told to run, as its inline config
/// (`OPENCODE_CONFIG_CONTENT`): `tag`'s profile for the work and for the
/// session titles, so neither goes to a provider, served by the daemon at
/// `base` — the one ket checked is on this Mac. OpenCode 2's interactive
/// command has no model flag; Ollama's own `ollama launch opencode` passes
/// the model the same way. The address is named too, because a config of the
/// person's own can point OpenCode's `ollama` provider at another machine,
/// and this config is the one OpenCode reads last. A provider policy also
/// keeps configured subagents from choosing a cloud model.
pub fn opencode_config(tag: &str, base: &str) -> String {
    let model = format!("ollama/{}", profile_tag(tag));
    serde_json::json!({
        "model": model,
        "small_model": model,
        "providers": { "ollama": { "settings": { "baseURL": format!("{base}/v1") } } },
        "policies": [
            { "action": "provider.use", "resource": "*", "effect": "deny" },
            { "action": "provider.use", "resource": "ollama", "effect": "allow" },
        ],
    })
    .to_string()
}

/// Whether a name says the model runs in Ollama's cloud.
pub fn is_cloud_tag(tag: &str) -> bool {
    tag.ends_with(":cloud") || tag.ends_with("-cloud")
}

/// How much memory a Mac has, in the steps models are sized for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Under 24 GB.
    Gb16,
    /// 24 to 48 GB.
    Gb32,
    /// 48 to 96 GB.
    Gb64,
    /// 96 GB and up.
    Gb128,
}

impl Tier {
    /// What a person reads: "64 GB".
    pub fn label(self) -> &'static str {
        match self {
            Tier::Gb16 => "16 GB",
            Tier::Gb32 => "32 GB",
            Tier::Gb64 => "64 GB",
            Tier::Gb128 => "128 GB",
        }
    }
}

const GB: u64 = 1024 * 1024 * 1024;

/// The tier `total` bytes of memory fall in.
pub fn tier(total: u64) -> Tier {
    match total {
        t if t < 24 * GB => Tier::Gb16,
        t if t < 48 * GB => Tier::Gb32,
        t if t < 96 * GB => Tier::Gb64,
        _ => Tier::Gb128,
    }
}

/// A model worth offering to pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recommended {
    /// What `ollama pull` takes.
    pub tag: &'static str,
    /// About how much it downloads, and holds once loaded.
    pub size_bytes: u64,
    /// One line on why.
    pub note: &'static str,
    /// The smallest Mac it is worth offering on.
    pub min_tier: Tier,
}

const fn gb_tenths(tenths: u64) -> u64 {
    tenths * GB / 10
}

/// Coding models that call tools, from Ollama's library, October 2026, in
/// the order ket recommends them: the first a Mac has room for is its best
/// fit. Ranked by how well they code, not how fast — by SWE-bench Verified on
/// Vals.ai's independent harness where a model has a score there. Sizes are
/// the Q4 downloads.
pub const RECOMMENDED: &[Recommended] = &[
    Recommended {
        tag: "qwen3.8:27b",
        size_bytes: gb_tenths(165),
        note: "The strongest coder that runs on a Mac. Slower, and careful.",
        min_tier: Tier::Gb32,
    },
    Recommended {
        tag: "qwen3.6:35b-a3b-coding",
        size_bytes: gb_tenths(235),
        note: "Tuned for code, and much quicker to answer.",
        min_tier: Tier::Gb64,
    },
    Recommended {
        tag: "glm-4.7-flash",
        size_bytes: gb_tenths(190),
        note: "Strong at agentic tool use, and quick for its size.",
        min_tier: Tier::Gb32,
    },
    Recommended {
        tag: "qwen3-coder:30b",
        size_bytes: gb_tenths(190),
        note: "Tuned for code, and the fastest coder for its memory.",
        min_tier: Tier::Gb32,
    },
    Recommended {
        tag: "gemma4:12b",
        size_bytes: gb_tenths(80),
        note: "Small and fast. The pick for a lighter Mac or quick jobs.",
        min_tier: Tier::Gb16,
    },
    Recommended {
        tag: "gpt-oss:20b",
        size_bytes: gb_tenths(140),
        note: "Quick for its size, and good at reasoning through a task.",
        min_tier: Tier::Gb16,
    },
];

/// Whose model it is, for the logo beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Alibaba's Qwen.
    Qwen,
    /// Google's Gemma.
    Gemma,
    /// Zhipu's GLM.
    Glm,
    /// OpenAI's gpt-oss.
    OpenAi,
    /// Meta's Llama.
    Meta,
    /// Mistral's models, Devstral among them.
    Mistral,
    /// DeepSeek's.
    DeepSeek,
    /// Microsoft's Phi.
    Microsoft,
    /// Anyone else.
    Other,
}

/// The family `tag` belongs to, from its name.
pub fn family(tag: &str) -> Family {
    let base = tag
        .split([':', '/'])
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase();
    let starts = |prefix: &str| base.starts_with(prefix);
    if starts("qwen") || starts("qwq") {
        Family::Qwen
    } else if starts("gemma") {
        Family::Gemma
    } else if starts("glm") || starts("chatglm") {
        Family::Glm
    } else if starts("gpt-oss") {
        Family::OpenAi
    } else if starts("llama") || starts("codellama") {
        Family::Meta
    } else if starts("mistral") || starts("devstral") || starts("codestral") || starts("magistral")
    {
        Family::Mistral
    } else if starts("deepseek") {
        Family::DeepSeek
    } else if starts("phi") {
        Family::Microsoft
    } else {
        Family::Other
    }
}

/// How a model reads to a person: `Qwen3 Coder` and `30B` for
/// `qwen3-coder:30b`, `Gemma 4` and `12B` for `gemma4:12b`. The tag itself is
/// still shown beside it, small, for anyone who wants the exact name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pretty {
    /// The name.
    pub name: String,
    /// Its size class, when the tag says: `30B`, `0.6B`.
    pub size: Option<String>,
}

/// [`Pretty`] for `tag`.
pub fn pretty(tag: &str) -> Pretty {
    let (base, variant) = tag.split_once(':').unwrap_or((tag, ""));
    let base = base.rsplit('/').next().unwrap_or(base);
    let mut name = match base.to_ascii_lowercase().as_str() {
        "gpt-oss" => "gpt-oss".to_owned(),
        _ => base
            .split('-')
            .map(word)
            .collect::<Vec<_>>()
            .join(" ")
            .replace("GLM ", "GLM-"),
    };
    let mut size = None;
    for part in variant.split('-').filter(|part| !part.is_empty()) {
        let lower = part.to_ascii_lowercase();
        let is_size = lower.len() > 1
            && (lower.ends_with('b') || lower.ends_with('m'))
            && lower[..lower.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.');
        if is_size && size.is_none() {
            size = Some(lower.to_ascii_uppercase());
        } else if matches!(lower.as_str(), "coding" | "instruct" | "thinking") {
            name.push(' ');
            name.push_str(&word(part));
        }
    }
    Pretty { name, size }
}

/// One word of a model's name, as its makers write it: `qwen3` → `Qwen3`,
/// `gemma4` → `Gemma 4`, `llama3.2` → `Llama 3.2`, `coder` → `Coder`.
fn word(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    // A number glued to a few names reads apart: Gemma 4, Llama 3.2, Phi 4.
    for spaced in ["gemma", "llama", "phi", "mistral"] {
        if let Some(rest) = lower.strip_prefix(spaced)
            && rest.starts_with(|c: char| c.is_ascii_digit())
        {
            return format!("{} {rest}", capitalise(spaced));
        }
    }
    match lower.as_str() {
        "deepseek" => "DeepSeek".to_owned(),
        "glm" => "GLM".to_owned(),
        _ => capitalise(&lower),
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How a model sits in a Mac's memory at a context length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Leaves room for the editor, builds and everything else.
    Fits,
    /// Runs, with little left over.
    Tight,
    /// Would push the Mac into swap.
    TooBig,
}

/// KV cache bytes at `context` tokens: the per-token figure, plus a tenth for
/// what Ollama allocates around it.
pub fn kv_bytes(context: u32, kv_per_token: Option<u64>) -> u64 {
    let per_token = kv_per_token.unwrap_or(DEFAULT_KV_PER_TOKEN);
    u64::from(context) * per_token * 11 / 10
}

/// How `model_bytes` of weights plus `kv` bytes of cache sit in `total`:
/// fits up to 60% of memory, tight up to 70%.
pub fn fit(model_bytes: u64, kv: u64, total: u64) -> Fit {
    let need = model_bytes.saturating_add(kv);
    if need <= total / 10 * 6 {
        Fit::Fits
    } else if need <= total / 10 * 7 {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

/// The context ket gives a model: the longest of 64k, 32k and 16k that fits
/// with room to spare, else the longest that fits at all, never past the
/// model's own maximum. `None` when it fits at none of them, or the model
/// cannot take [`MIN_CONTEXT`].
pub fn context_for(
    model_bytes: u64,
    kv_per_token: Option<u64>,
    context_max: Option<u32>,
    total: u64,
) -> Option<u32> {
    let cap = context_max.unwrap_or(u32::MAX);
    if cap < MIN_CONTEXT {
        return None;
    }
    let candidates: Vec<u32> = CONTEXTS.iter().map(|c| (*c).min(cap)).collect();
    let at = |wanted: Fit| {
        candidates
            .iter()
            .copied()
            .find(|context| fit(model_bytes, kv_bytes(*context, kv_per_token), total) == wanted)
    };
    at(Fit::Fits).or_else(|| at(Fit::Tight))
}

/// Why a model is not an agent, when it is not — what a settings row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// An agent, at this context.
    Ready(u32),
    /// Could be one: its profile is missing or out of date, and preparing it
    /// at this context makes it one.
    Unprepared(u32),
    /// Runs in Ollama's cloud.
    Cloud,
    /// Served from another machine.
    Remote,
    /// Cannot call tools.
    NoTools,
    /// Too big for this Mac at any context ket offers, or its own context is
    /// too short.
    TooBig,
    /// The name its profile would take already belongs to a model ket did not
    /// make from it. Preparing would replace that model, so it is left alone.
    Taken,
}

/// Where `model` stands as a local agent on a Mac with `total` bytes of
/// memory, given everything the daemon listed.
pub fn standing(snapshot: &Snapshot, model: &ModelInfo, total: u64) -> Standing {
    if model.cloud {
        return Standing::Cloud;
    }
    if !snapshot.endpoint.is_loopback() {
        return Standing::Remote;
    }
    if !model.tools {
        return Standing::NoTools;
    }
    let Some(wanted) = context_for(
        model.size_bytes,
        model.kv_per_token,
        model.context_max,
        total,
    ) else {
        return Standing::TooBig;
    };
    let profile = profile_tag(&model.tag);
    if snapshot
        .models
        .iter()
        .any(|other| other.tag == profile && other.parent.as_deref() != Some(model.tag.as_str()))
    {
        return Standing::Taken;
    }
    match snapshot
        .profile_of(&model.tag)
        .and_then(|profile| profile.num_ctx)
    {
        Some(pinned) if pinned == wanted => Standing::Ready(pinned),
        _ => Standing::Unprepared(wanted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::{Endpoint, ModelInfo, Snapshot};

    const GB64: u64 = 64 * GB;

    #[test]
    fn names_round_trip() {
        let agent = agent_name("qwen3-coder:30b");
        assert_eq!(agent, "opencode:ollama/qwen3-coder:30b");
        assert_eq!(model_tag(&agent), Some("qwen3-coder:30b"));
        assert_eq!(model_tag("opencode"), None);
        assert_eq!(model_tag("ollama:qwen3"), None);
        assert_eq!(model_tag(AGENT_PREFIX), None);
        assert_eq!(profile_tag("qwen3:0.6b"), "qwen3:0.6b-ket");
        assert!(is_cloud_tag("gpt-oss:20b-cloud"));
        assert!(is_cloud_tag("gemma4:cloud"));
        assert!(!is_cloud_tag("qwen3-coder:30b"));
    }

    #[test]
    fn models_read_the_way_their_makers_write_them() {
        let read = |tag: &str| {
            let p = pretty(tag);
            (p.name, p.size)
        };
        let some = |s: &str| Some(s.to_owned());
        assert_eq!(
            read("qwen3-coder:30b"),
            ("Qwen3 Coder".to_owned(), some("30B"))
        );
        assert_eq!(read("qwen3:0.6b"), ("Qwen3".to_owned(), some("0.6B")));
        assert_eq!(read("qwen3.8:27b"), ("Qwen3.8".to_owned(), some("27B")));
        assert_eq!(
            read("qwen3.6:27b-coding"),
            ("Qwen3.6 Coding".to_owned(), some("27B"))
        );
        assert_eq!(
            read("qwen3.6:35b-a3b-coding"),
            ("Qwen3.6 Coding".to_owned(), some("35B"))
        );
        assert_eq!(read("gemma4:12b"), ("Gemma 4".to_owned(), some("12B")));
        assert_eq!(read("glm-4.7-flash"), ("GLM-4.7 Flash".to_owned(), None));
        assert_eq!(
            read("gpt-oss:20b-cloud"),
            ("gpt-oss".to_owned(), some("20B"))
        );
        assert_eq!(read("llama3.2:3b"), ("Llama 3.2".to_owned(), some("3B")));
        assert_eq!(
            read("deepseek-r1:8b"),
            ("DeepSeek R1".to_owned(), some("8B"))
        );
        assert_eq!(
            read("hf.co/someone/model:latest"),
            ("Model".to_owned(), None)
        );
        assert_eq!(family("qwen3-coder:30b"), Family::Qwen);
        assert_eq!(family("glm-4.7-flash"), Family::Glm);
        assert_eq!(family("gpt-oss:120b"), Family::OpenAi);
        assert_eq!(family("devstral:24b"), Family::Mistral);
        assert_eq!(family("mystery:1b"), Family::Other);
    }

    #[test]
    fn tiers_step_at_24_48_and_96_gb() {
        assert_eq!(tier(16 * GB), Tier::Gb16);
        assert_eq!(tier(24 * GB - 1), Tier::Gb16);
        assert_eq!(tier(24 * GB), Tier::Gb32);
        assert_eq!(tier(48 * GB), Tier::Gb64);
        assert_eq!(tier(GB64), Tier::Gb64);
        assert_eq!(tier(96 * GB), Tier::Gb128);
    }

    #[test]
    fn fit_draws_its_lines_at_sixty_and_seventy_percent() {
        let total = 100 * GB;
        assert_eq!(fit(50 * GB, 10 * GB, total), Fit::Fits);
        assert_eq!(fit(50 * GB, 15 * GB, total), Fit::Tight);
        assert_eq!(fit(60 * GB, 10 * GB, total), Fit::Tight);
        assert_eq!(fit(60 * GB, 11 * GB, total), Fit::TooBig);
        // A tenth on top of the per-token figure.
        assert_eq!(kv_bytes(1000, Some(100)), 110_000);
        assert_eq!(kv_bytes(1, None), DEFAULT_KV_PER_TOKEN * 11 / 10);
    }

    #[test]
    fn context_steps_down_to_what_fits_and_never_past_the_model() {
        // 19 GB of weights on 64 GB: 64k fits.
        assert_eq!(context_for(19 * GB, Some(98_304), None, GB64), Some(65_536));
        // The model's own maximum caps it.
        assert_eq!(
            context_for(GB / 2, Some(114_688), Some(40_960), GB64),
            Some(40_960)
        );
        // A big model on a small Mac steps down: 7 GB on 16 GB takes 16k.
        assert_eq!(context_for(7 * GB, None, None, 16 * GB), Some(16_384));
        // Too big at every length.
        assert_eq!(context_for(65 * GB, None, None, GB64), None);
        // A model that cannot take 16k is never offered.
        assert_eq!(context_for(GB, None, Some(8_192), GB64), None);
    }

    fn model(tag: &str) -> ModelInfo {
        ModelInfo {
            tag: tag.to_owned(),
            size_bytes: GB / 2,
            family: None,
            parameters: None,
            quantization: None,
            cloud: false,
            tools: true,
            context_max: Some(40_960),
            parent: None,
            num_ctx: None,
            kv_per_token: Some(114_688),
        }
    }

    fn profile(of: &str, num_ctx: u32) -> ModelInfo {
        ModelInfo {
            parent: Some(of.to_owned()),
            num_ctx: Some(num_ctx),
            ..model(&profile_tag(of))
        }
    }

    fn snapshot(models: Vec<ModelInfo>, host: Option<&str>) -> Snapshot {
        Snapshot {
            endpoint: match host {
                None => Endpoint {
                    base: "http://127.0.0.1:11434".to_owned(),
                },
                Some(base) => Endpoint {
                    base: base.to_owned(),
                },
            },
            version: None,
            models,
            loaded: Vec::new(),
        }
    }

    #[test]
    fn a_model_is_an_agent_only_once_its_profile_matches_what_it_fits_at() {
        let base = model("qwen3:0.6b");
        let none = snapshot(vec![base.clone()], None);
        assert_eq!(standing(&none, &base, GB64), Standing::Unprepared(40_960));
        let stale = snapshot(vec![base.clone(), profile("qwen3:0.6b", 16_384)], None);
        assert_eq!(standing(&stale, &base, GB64), Standing::Unprepared(40_960));
        let ready = snapshot(vec![base.clone(), profile("qwen3:0.6b", 40_960)], None);
        assert_eq!(standing(&ready, &base, GB64), Standing::Ready(40_960));
    }

    #[test]
    fn a_model_of_the_users_under_the_profile_name_is_never_prepared_over() {
        let base = model("qwen3:0.6b");
        let theirs = ModelInfo {
            parent: Some("llama3.2:3b".to_owned()),
            ..model("qwen3:0.6b-ket")
        };
        let taken = snapshot(vec![base.clone(), theirs], None);
        assert_eq!(standing(&taken, &base, GB64), Standing::Taken);
        let other_weights = ModelInfo {
            size_bytes: 3 * GB,
            ..model("qwen3:0.6b-ket")
        };
        let unparented = snapshot(vec![base.clone(), other_weights], None);
        assert_eq!(standing(&unparented, &base, GB64), Standing::Taken);
    }

    #[test]
    fn a_copy_that_records_no_parent_is_never_prepared_over_or_listed() {
        // Ollama records no parent on a copy of a safetensors model, and a
        // copy ket has no record of is the person's however alike it looks.
        let base = model("qwen3.8:27b");
        let copy = ModelInfo {
            num_ctx: Some(40_960),
            ..model("qwen3.8:27b-ket")
        };
        let snap = snapshot(vec![base.clone(), copy], None);
        assert_eq!(standing(&snap, &base, GB64), Standing::Taken);
        // Never offered as a model of its own, so never `-ket-ket`.
        assert_eq!(
            snap.listed().map(|m| m.tag.as_str()).collect::<Vec<_>>(),
            vec!["qwen3.8:27b"]
        );
    }

    #[test]
    fn cloud_remote_toolless_and_oversized_models_are_never_agents() {
        let cloud = ModelInfo {
            cloud: true,
            ..model("gpt-oss:20b-cloud")
        };
        let toolless = ModelInfo {
            tools: false,
            ..model("llama3.2:3b")
        };
        let huge = ModelInfo {
            size_bytes: 65 * GB,
            ..model("gpt-oss:120b")
        };
        let local = snapshot(vec![], None);
        assert_eq!(standing(&local, &cloud, GB64), Standing::Cloud);
        assert_eq!(standing(&local, &toolless, GB64), Standing::NoTools);
        assert_eq!(standing(&local, &huge, GB64), Standing::TooBig);
        let remote = snapshot(vec![], Some("http://gpu-box.local:11434"));
        assert_eq!(
            standing(&remote, &model("qwen3:0.6b"), GB64),
            Standing::Remote
        );
    }
}
