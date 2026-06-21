#!/usr/bin/env bash
# Launch several Claude Code agents in a tmux grid — one per pane, each in its
# own isolated git worktree (via `claude --worktree`) so they never collide.
#
# Usage:
#   scripts/swarm.sh                         # 2 default lanes
#   scripts/swarm.sh frontend backend ui     # one pane per name
#   SWARM_YOLO=1 scripts/swarm.sh a b         # run agents with permissions bypassed (autonomous)
#
# Then:  tmux attach -t swarm     (switch panes: Ctrl-b <arrow>, detach: Ctrl-b d)
set -euo pipefail

session="${SWARM_SESSION:-swarm}"
lanes=("$@"); [ ${#lanes[@]} -eq 0 ] && lanes=(lane-a lane-b)
flags=""; [ "${SWARM_YOLO:-0}" = "1" ] && flags="--dangerously-skip-permissions"

command -v tmux   >/dev/null || { echo "tmux not installed (brew install tmux)"; exit 1; }
command -v claude >/dev/null || { echo "claude CLI not on PATH"; exit 1; }
tmux has-session -t "$session" 2>/dev/null && { echo "Session '$session' already exists -> tmux attach -t $session"; exit 0; }

# Create the session (first pane), then split for the rest, capturing pane ids.
tmux new-session -d -s "$session" -n agents
ids=("$(tmux list-panes -t "$session:agents" -F '#{pane_id}')")
for ((i=1; i<${#lanes[@]}; i++)); do
  ids+=("$(tmux split-window -t "$session:agents" -P -F '#{pane_id}')")
  tmux select-layout -t "$session:agents" tiled >/dev/null
done
tmux select-layout -t "$session:agents" tiled >/dev/null
tmux set -t "$session" pane-border-status top >/dev/null 2>&1 || true

# One claude agent per pane, each in its own worktree named after the lane.
for ((i=0; i<${#lanes[@]}; i++)); do
  tmux select-pane -t "${ids[$i]}" -T " ${lanes[$i]} " >/dev/null 2>&1 || true
  tmux send-keys  -t "${ids[$i]}" "claude --worktree ${lanes[$i]} ${flags}" C-m
done

echo "Launched ${#lanes[@]} agent(s) in tmux session '$session': ${lanes[*]}"
echo "Attach:  tmux attach -t $session"
echo "Kill:    tmux kill-session -t $session"
