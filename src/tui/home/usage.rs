//! Local session cost snapshots for the sidebar.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::session::Instance;
use crate::tui::components::{rendered_width, truncate_to_width};
use crate::tui::worker::Worker;

const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const TRANSCRIPT_TAIL_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    id: String,
    native_id: String,
    agent: String,
    tool: String,
    profile: String,
    store: Option<PathBuf>,
    sandboxed: bool,
}

impl Request {
    fn for_instance(instance: &Instance) -> Option<Self> {
        let native_id = instance.agent_session_id.as_deref()?;
        if !crate::session::capture::is_valid_session_id(native_id) {
            return None;
        }
        let execution = instance
            .agent_session_binding
            .as_ref()
            .filter(|binding| binding.session_id == native_id)
            .and_then(|binding| binding.execution.as_ref());
        let agent = execution.map_or(instance.tool.as_str(), |source| source.agent.as_str());
        if !matches!(agent, "claude" | "codex") {
            return None;
        }
        let store = if instance.is_sandboxed() {
            instance.capture_store_dir()
        } else {
            execution
                .filter(|source| source.filesystem == "host")
                .and_then(|source| source.stores.first().cloned())
        };
        Some(Self {
            id: instance.id.clone(),
            native_id: native_id.into(),
            agent: agent.into(),
            tool: instance.tool.clone(),
            profile: instance.effective_profile(),
            store,
            sandboxed: instance.is_sandboxed(),
        })
    }

    fn store(&self) -> Option<PathBuf> {
        if let Some(store) = &self.store {
            return Some(store.clone());
        }
        if self.sandboxed {
            return None;
        }
        let home = dirs::home_dir()?;
        let config = crate::session::resolve_config_or_warn(&self.profile);
        config
            .session
            .agent_config_dir_for(&self.tool, &home)
            .or_else(|| {
                std::env::var_os(if self.agent == "claude" {
                    "CLAUDE_CONFIG_DIR"
                } else {
                    "CODEX_HOME"
                })
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
            })
            .or_else(|| Some(home.join(format!(".{}", self.agent))))
    }
}

type Snapshots = HashMap<String, (Request, Option<f64>, Option<u64>)>;

pub(super) struct UsagePoller {
    worker: Worker<Vec<Request>, Snapshots>,
    snapshots: Snapshots,
    last_refresh: Option<Instant>,
    pending: bool,
}

impl UsagePoller {
    pub(super) fn new() -> Self {
        Self {
            worker: Worker::spawn("session-usage", collect),
            snapshots: HashMap::new(),
            last_refresh: None,
            pending: false,
        }
    }

    pub(super) fn refresh(&mut self, instances: impl Iterator<Item = Instance>) {
        if let Ok(snapshots) = self.worker.try_recv() {
            self.snapshots = snapshots;
            self.pending = false;
            self.last_refresh = Some(Instant::now());
        }
        if self.pending
            || self
                .last_refresh
                .is_some_and(|at| at.elapsed() < REFRESH_INTERVAL)
        {
            return;
        }
        let requests: Vec<_> = instances
            .filter_map(|instance| Request::for_instance(&instance))
            .collect();
        if requests.is_empty() {
            return;
        }
        self.worker.request(requests);
        self.pending = true;
    }

    pub(super) fn label(&self, instance: &Instance, width: usize) -> Option<String> {
        let request = Request::for_instance(instance)?;
        let (cost, context) = self
            .snapshots
            .get(&instance.id)
            .filter(|(original, ..)| &request == original)
            .map_or((None, None), |(_, cost, context)| (*cost, *context));
        let cost = cost.map_or_else(|| "cost ?".into(), |cost| format!("${cost:.2}"));
        let with_context = context.map(|percent| format!("{cost} {percent}%"));
        let label = with_context
            .filter(|label| rendered_width(label) <= width)
            .unwrap_or(cost);
        Some(truncate_to_width(&label, width))
    }

    #[cfg(test)]
    pub(super) fn seed_for_test(&mut self, instance: &Instance, cost: f64, context: Option<u64>) {
        self.snapshots.insert(
            instance.id.clone(),
            (
                Request::for_instance(instance).unwrap(),
                Some(cost),
                context,
            ),
        );
    }
}

#[derive(Deserialize)]
struct Report {
    session: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    agent: String,
    period: String,
    total_cost: Option<f64>,
    #[serde(default)]
    model_breakdowns: Vec<Pricing>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pricing {
    #[serde(default)]
    missing_pricing: bool,
}

impl Entry {
    fn native_id(&self) -> Option<&str> {
        if self.agent == "codex" {
            let id = self.period.get(self.period.len().checked_sub(36)?..)?;
            uuid::Uuid::parse_str(id).ok()?;
            Some(id)
        } else {
            Some(&self.period)
        }
    }

    fn cost(&self) -> Option<f64> {
        self.total_cost.filter(|cost| {
            cost.is_finite()
                && *cost >= 0.0
                && !self
                    .model_breakdowns
                    .iter()
                    .any(|model| model.missing_pricing)
        })
    }
}

fn report(store: &Path, agent: &str) -> anyhow::Result<Report> {
    let mut command = Command::new("npx");
    command.args([
        "--offline",
        "--yes",
        "ccusage@20.0.40",
        "session",
        "--json",
        "--offline",
    ]);
    command.env(
        if agent == "claude" {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        },
        store,
    );
    let output =
        crate::process::run_with_timeout_process_group(&mut command, Duration::from_secs(30))?
            .ok_or_else(|| anyhow::anyhow!("ccusage timed out"))?;
    anyhow::ensure!(
        output.status.success(),
        "ccusage exited with {}",
        output.status
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptLine {
    #[serde(default)]
    is_sidechain: bool,
    message: Option<TranscriptMessage>,
}

#[derive(Deserialize)]
struct TranscriptMessage {
    model: Option<String>,
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Share of the model's window that the conversation's latest main-thread reply read, in percent.
fn context_percent(store: &Path, native_id: &str) -> Option<u64> {
    let transcript = std::fs::read_dir(store.join("projects"))
        .ok()?
        .filter_map(Result::ok)
        .map(|project| project.path().join(format!("{native_id}.jsonl")))
        .find(|path| path.is_file())?;
    let mut file = File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_BYTES)))
        .ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let (model, tokens) = String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .find_map(|line| {
            let line: TranscriptLine = serde_json::from_str(line).ok()?;
            let message = line.message.filter(|_| !line.is_sidechain)?;
            let usage = message.usage?;
            let tokens = usage.input_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens;
            (tokens > 0).then(|| (message.model.unwrap_or_default(), tokens))
        })?;
    let window = context_window(&model);
    Some((tokens * 100 + window / 2) / window)
}

// shortcut: 5.x models get 1M and older ones 200k, as ccusage assumes; a model with another window needs its own rule.
fn context_window(model: &str) -> u64 {
    let mut parts = model.split('-').skip(1);
    let major = match (parts.next(), parts.next()) {
        (Some(family), Some(major)) if family.parse::<u32>().is_err() => major.parse::<u32>().ok(),
        _ => None,
    };
    if major.is_some_and(|major| major >= 5) {
        1_000_000
    } else {
        200_000
    }
}

fn collect(requests: Vec<Request>) -> Snapshots {
    collect_with(requests, report)
}

fn collect_with(
    requests: Vec<Request>,
    read_report: impl Fn(&Path, &str) -> anyhow::Result<Report>,
) -> Snapshots {
    let mut groups: HashMap<(PathBuf, String), Vec<Request>> = HashMap::new();
    let mut snapshots = HashMap::new();
    for request in requests {
        if let Some(store) = request.store().and_then(|store| store.canonicalize().ok()) {
            groups
                .entry((store, request.agent.clone()))
                .or_default()
                .push(request);
        }
    }
    for ((store, agent), requests) in groups {
        let entries = match read_report(&store, &agent) {
            Ok(report) => report.session,
            Err(error) => {
                tracing::warn!(target: "tui.usage", %error, "Cannot refresh session costs");
                Vec::new()
            }
        };
        for request in requests {
            let cost = entries
                .iter()
                .find(|entry| entry.agent == agent && entry.native_id() == Some(&request.native_id))
                .and_then(Entry::cost);
            let context = (agent == "claude")
                .then(|| context_percent(&store, &request.native_id))
                .flatten();
            snapshots.insert(request.id.clone(), (request, cost, context));
        }
    }
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";

    fn instance(root: &Path, agent: &str) -> Instance {
        let mut instance = Instance::new("fix-login", "/tmp/project");
        instance.source_profile = "test".into();
        instance.tool = agent.into();
        instance.agent_session_id = Some(SID.into());
        instance.agent_session_binding = Some(crate::session::ConversationBinding {
            session_id: SID.into(),
            execution: Some(crate::session::ExecutionBinding {
                agent: agent.into(),
                stores: vec![root.into()],
                configuration: Vec::new(),
                cwd: "/tmp/project".into(),
                cwd_filesystem: "host".into(),
                filesystem: "host".into(),
                exported_default_store: None,
            }),
            provenance: crate::session::ConversationProvenance::Observed,
            transcript_path: None,
        });
        instance
    }

    fn entry(agent: &str) -> Entry {
        Entry {
            agent: agent.into(),
            period: if agent == "codex" {
                format!("2026/10/22/rollout-2026-10-22T09-00-00-{SID}")
            } else {
                SID.into()
            },
            total_cost: Some(4.32),
            model_breakdowns: Vec::new(),
        }
    }

    #[test]
    fn cost_follows_the_conversation_and_its_store() {
        for agent in ["claude", "codex"] {
            let root = tempfile::tempdir().unwrap();
            let mut instance = instance(root.path(), agent);
            let request = Request::for_instance(&instance).unwrap();
            let mut poller = UsagePoller::new();
            for cost in [4.32, 5.67] {
                poller.snapshots = collect_with(vec![request.clone()], |store, reported_agent| {
                    assert_eq!(store, root.path().canonicalize().unwrap());
                    assert_eq!(reported_agent, agent);
                    let mut current = entry(agent);
                    current.total_cost = Some(cost);
                    Ok(Report {
                        session: vec![current],
                    })
                });
                let expected = format!("${cost:.2}");
                assert_eq!(poller.label(&instance, 80).unwrap(), expected);
                assert_eq!(poller.label(&instance, expected.len()).unwrap(), expected);
                assert!(rendered_width(&poller.label(&instance, 3).unwrap()) <= 3);
            }
            instance.agent_session_id = Some("new-conversation".into());
            assert_eq!(poller.label(&instance, 80).unwrap(), "cost ?");
            instance.agent_session_id = Some(SID.into());
            instance
                .agent_session_binding
                .as_mut()
                .unwrap()
                .execution
                .as_mut()
                .unwrap()
                .stores = vec!["/another-account".into()];
            assert_eq!(poller.label(&instance, 80).unwrap(), "cost ?");
        }
    }

    #[test]
    fn report_distinguishes_missing_prices_from_zero_cost() {
        let report: Report = serde_json::from_value(serde_json::json!({"session":[
            {"agent":"claude","period":SID,"totalCost":0,"modelBreakdowns":[]},
            {"agent":"claude","period":SID,"totalCost":0,"modelBreakdowns":[{"missingPricing":true}]},
            {"agent":"claude","period":SID,"totalCost":-1},
            {"agent":"claude","period":SID}
        ]})).unwrap();
        assert_eq!(
            report.session.iter().map(Entry::cost).collect::<Vec<_>>(),
            vec![Some(0.0), None, None, None]
        );
        assert_eq!(entry("codex").native_id(), Some(SID));
        let mut invalid = entry("codex");
        invalid.period = "not-a-session-id".into();
        assert_eq!(invalid.native_id(), None);
    }

    #[test]
    fn context_follows_the_latest_main_reply_and_yields_to_the_cost() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/-tmp-project");
        std::fs::create_dir_all(&project).unwrap();
        let reply = |model: &str, sidechain: bool, input: u64, created: u64, read: u64| {
            serde_json::json!({"type": "assistant", "isSidechain": sidechain, "message": {
                "model": model,
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": created,
                    "cache_read_input_tokens": read
                }
            }})
            .to_string()
        };
        let lines = [
            reply("claude-sonnet-5-5", false, 2, 100, 900_000),
            reply("claude-sonnet-5-5", false, 2, 356, 458_739),
            reply("claude-sonnet-5-5", true, 2, 0, 900_000),
            reply("<synthetic>", false, 0, 0, 0),
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#.into(),
        ];
        std::fs::write(project.join(format!("{SID}.jsonl")), lines.join("\n")).unwrap();
        assert_eq!(context_percent(root.path(), SID), Some(46));
        assert_eq!(context_percent(root.path(), "other-conversation"), None);

        let instance = instance(root.path(), "claude");
        let mut poller = UsagePoller::new();
        poller.snapshots = collect_with(vec![Request::for_instance(&instance).unwrap()], |_, _| {
            Ok(Report {
                session: vec![entry("claude")],
            })
        });
        assert_eq!(poller.label(&instance, 80).unwrap(), "$4.32 46%");
        assert_eq!(poller.label(&instance, 8).unwrap(), "$4.32");
    }

    #[test]
    fn context_window_follows_the_model_generation() {
        for (model, window) in [
            ("claude-sonnet-5-5", 1_000_000),
            ("claude-fable-5-1", 1_000_000),
            ("claude-sonnet-4-5", 200_000),
            ("claude-3-5-sonnet-20241022", 200_000),
            ("", 200_000),
        ] {
            assert_eq!(context_window(model), window, "{model}");
        }
    }
}
