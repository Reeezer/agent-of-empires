//! Claude sessions that stopped to wait on something they started, so the row is not read as waiting on the user.
//!
//! An idle session waits when its shell tool still has a command running. A session aoe shows as
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

/// Instance id, tmux session name and whether the row was idle when asked.
type Requests = Vec<(String, String, bool)>;

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
            .filter(|instance| {
                instance.tool == "claude"
                    && matches!(instance.status, Status::Idle | Status::Running)
            })
            .map(|instance| {
                (
                    instance.id.clone(),
                    crate::tmux::Session::generate_name(&instance.id, &instance.title),
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

fn monitor_armed(session: &str) -> bool {
    crate::tmux::tmux_command()
        .args(["capture-pane", "-p", "-t", &format!("{session}:^.0")])
        .args(["-S", &format!("-{CAPTURE_LINES}")])
        .output()
        .is_ok_and(|output| screen_arms_monitor(&String::from_utf8_lossy(&output.stdout)))
}

fn collect(requests: Requests) -> HashMap<String, bool> {
    let panes: Vec<_> = requests
        .iter()
        .filter_map(|(id, session, idle)| {
            Some((id, session, *idle, crate::process::get_pane_pid(session)?))
        })
        .collect();
    let roots: Vec<u32> = panes.iter().map(|(.., pid)| *pid).collect();
    let trees = crate::process::descendant_command_lines(&roots);
    panes
        .into_iter()
        .filter(|(_, session, idle, pid)| {
            if *idle {
                trees
                    .get(pid)
                    .is_some_and(|lines| runs_bash_tool_command(lines))
            } else {
                monitor_armed(session)
            }
        })
        .map(|(id, _, idle, _)| (id.clone(), idle))
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
