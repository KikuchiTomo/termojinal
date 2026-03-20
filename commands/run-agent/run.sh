#!/usr/bin/env bash
set -euo pipefail

# Run AI Agent - select an AI agent and launch it in a specified directory.

# Step 1: Select an agent
items='[
    {"value":"claude","label":"Claude Code","description":"Anthropic Claude Code CLI agent"},
    {"value":"codex","label":"Codex CLI","description":"OpenAI Codex CLI agent"},
    {"value":"aider","label":"Aider","description":"AI pair programming in terminal"}
]'

echo "{\"type\":\"fuzzy\",\"prompt\":\"Select AI agent to launch\",\"items\":$items}"

read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

agent=$(echo "$response" | jq -r '.value // empty')
if [ -z "$agent" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Step 2: Ask for working directory
echo '{"type":"text","label":"Working directory","placeholder":"current directory","default":"."}'
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

workdir=$(echo "$response" | jq -r '.value // empty')
workdir="${workdir/#\~/$HOME}"
[ -z "$workdir" ] && workdir="."

# Map agent to command name
case "$agent" in
    claude) cmd="claude" ;;
    codex)  cmd="codex" ;;
    aider)  cmd="aider" ;;
    *)      echo "{\"type\":\"error\",\"message\":\"Unknown agent: $agent\"}"; exit 1 ;;
esac

# Check if the agent is installed
if ! command -v "$cmd" &>/dev/null; then
    echo "{\"type\":\"error\",\"message\":\"$cmd is not installed. Install it first.\"}"
    exit 1
fi

echo "{\"type\":\"done\",\"notify\":\"Launching $cmd in $workdir\"}"
