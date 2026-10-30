//! Sessions that stopped to wait on something they started, so the row is not read as waiting on the user.
//!
//! An idle Claude session waits when its shell tool still has a command running, and an idle Codex
//! session when it lists background terminals above its prompt. A Claude session aoe shows as
//! running waits when the status line above the prompt reports an armed monitor; while the agent
//! works, that line is its own spinner instead, so a working agent stays green.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::session::{Instance, Status};
use crate::tmux::detect::{detect, Observation};
use crate::tui::worker::Worker;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Claude's Bash tool runs every command through a shell that sources a file from this directory.
const BASH_TOOL_MARKER: &str = "/shell-snapshots/";
const MONITOR_RULE: &str = "background_monitor";
const CAPTURE_LINES: u32 = 50;

/// Instance id, tmux session name, tool and whether the row was idle when asked.
type Requests = Vec<(String, String, String, bool)>;

pub(super) struct BackgroundPoller {
    worker: Worker<Requests, HashMap<String, bool>>,
    /// Watching instance ids, each with the idle flag it was checked under.
    watching: HashMap<String, bool>,
    last_refresh: Option<Instant>,
    pending: bool,
}

impl BackgroundPoller {
    pub(super) fn new() -> Self {
        Self {
            worker: Worker::spawn("session-background", collect),
            watching: HashMap::new(),
            last_refresh: None,
            pending: false,
        }
    }

    pub(super) fn refresh<'a>(&mut self, instances: impl Iterator<Item = &'a Instance>) {
        if let Ok(watching) = self.worker.try_recv() {
            self.watching = watching;
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
        let requests: Requests = instances
            .filter(|instance| match instance.tool.as_str() {
                "claude" => matches!(instance.status, Status::Idle | Status::Running),
                "codex" => matches!(instance.status, Status::Idle),
                _ => false,
            })
            .map(|instance| {
                (
                    instance.id.clone(),
                    crate::tmux::Session::generate_name(&instance.id, &instance.title),
                    instance.tool.clone(),
                    matches!(instance.status, Status::Idle),
                )
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        self.worker.request(requests);
        self.pending = true;
    }

    /// A row that changed between idle and running since the check is not watching until the next one.
    pub(super) fn is_watching(&self, instance: &Instance) -> bool {
        matches!(instance.status, Status::Idle | Status::Running)
            && self.watching.get(&instance.id) == Some(&matches!(instance.status, Status::Idle))
    }

    #[cfg(test)]
    pub(super) fn seed_for_test(&mut self, id: &str, idle: bool) {
        self.watching.insert(id.into(), idle);
    }
}

fn runs_bash_tool_command(command_lines: &[String]) -> bool {
    command_lines
        .iter()
        .any(|line| line.contains(BASH_TOOL_MARKER))
}

fn screen_arms_monitor(screen: &str) -> bool {
    detect("claude", screen, &Observation::new("", None, true))
        .is_some_and(|detection| detection.rule == MONITOR_RULE)
}

/// Codex draws this row above its prompt only after the turn ends: while it works, the summary sits in the working line.
fn codex_terminal_running(screen: &str) -> bool {
    let mut above_prompt = screen
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .skip_while(|line| !line.trim_start().starts_with('\u{203A}'))
        .skip(1);
    above_prompt.next().is_some_and(|line| {
        line.trim_start()
            .split_once(' ')
            .is_some_and(|(count, rest)| {
                count.parse::<u32>().is_ok_and(|count| count > 0)
                    && rest.starts_with("background terminal")
            })
    })
}

fn capture(session: &str) -> String {
    crate::tmux::tmux_command()
        .args(["capture-pane", "-p", "-t", &format!("{session}:^.0")])
        .args(["-S", &format!("-{CAPTURE_LINES}")])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn collect(requests: Requests) -> HashMap<String, bool> {
    let panes: Vec<_> = requests
        .iter()
        .filter_map(|(id, session, tool, idle)| {
            Some((
                id,
                session,
                tool,
                *idle,
                crate::process::get_pane_pid(session)?,
            ))
        })
        .collect();
    let roots: Vec<u32> = panes.iter().map(|(.., pid)| *pid).collect();
    let trees = crate::process::descendant_command_lines(&roots);
    panes
        .into_iter()
        .filter(
            |(_, session, tool, idle, pid)| match (tool.as_str(), idle) {
                ("codex", _) => codex_terminal_running(&capture(session)),
                (_, true) => trees
                    .get(pid)
                    .is_some_and(|lines| runs_bash_tool_command(lines)),
                _ => screen_arms_monitor(&capture(session)),
            },
        )
        .map(|(id, _, _, idle, _)| (id.clone(), idle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bash_tool_counts_as_a_running_command() {
        let mcp = "github-mcp-server stdio --read-only".to_string();
        let hook = "sh -c [ -n \"$AOE_INSTANCE_ID\" ] || exit 0".to_string();
        let command = "/bin/zsh -c source /home/me/.claude/shell-snapshots/snapshot-zsh-1.sh && eval 'gh pr checks --watch'".to_string();
        for (lines, running) in [
            (vec![], false),
            (vec![mcp.clone(), hook.clone()], false),
            (vec![mcp, hook, command], true),
        ] {
            assert_eq!(runs_bash_tool_command(&lines), running, "{lines:?}");
        }
    }

    #[test]
    fn codex_waits_on_terminals_listed_above_its_prompt() {
        let prompt = "\u{203A} Ask Codex to do anything\n\n  GPT-6.1-Sol xhigh \u{b7} ~/project \u{b7} Main [default]\n  ? for shortcuts";
        for (name, screen, waiting) in [
            (
                "stopped with terminals running",
                format!("\u{2022} Done.\n\n  2 background terminals running \u{b7} /ps to view \u{b7} /stop to close\n\n{prompt}"),
                true,
            ),
            (
                "working with terminals running",
                format!("\u{2022} Working (34m 29s \u{2022} esc to interrupt) \u{b7} 1 background terminal running \u{b7} /ps to view \u{b7} /stop to close\n  \u{2514} Tip: Use /permissions.\n\n{prompt}"),
                false,
            ),
            (
                "stopped after quoting the row",
                format!("  1 background terminal running \u{b7} /ps to view\n\u{2022} That row means a command is still running.\n\n{prompt}"),
                false,
            ),
            ("no prompt", "  1 background terminal running".to_string(), false),
        ] {
            assert_eq!(codex_terminal_running(&screen), waiting, "{name}");
        }
    }

    /// The status line above the prompt decides: a working agent shows its own spinner there.
    #[test]
    fn a_monitor_only_counts_once_the_agent_has_stopped() {
        let chrome = "──────────────────────────────\n❯ \n──────────────────────────────\n  ⏵⏵ bypass permissions on · 1 monitor · ← for agents · ↓ to manage";
        for (name, status_line, armed) in [
            (
                "turn ended with a monitor armed",
                "✻ Worked for 5s · done 2:48 PM · 1 monitor still running",
                true,
            ),
            (
                "agent working with a monitor armed",
                "✻ Judging feedback… (22m 8s · ↓ 44.7k tokens)",
                false,
            ),
            ("turn ended, no monitor", "✻ Worked for 5s", false),
        ] {
            assert_eq!(
                screen_arms_monitor(&format!("{status_line}\n{chrome}")),
                armed,
                "{name}"
            );
        }
    }
}
