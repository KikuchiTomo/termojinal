#!/usr/bin/env bash
set -euo pipefail

# Switch Worktree - select from existing git worktrees and switch to one.

# List worktrees (excluding bare repos) with their branches
worktrees=$(git worktree list --porcelain | awk '/^worktree /{path=$2} /^branch /{branch=$2; gsub("refs/heads/","",branch); print path "\t" branch}')

if [ -z "$worktrees" ]; then
    echo '{"type":"info","message":"No worktrees found"}'
    sleep 1
    echo '{"type":"done"}'
    exit 0
fi

# Build fuzzy items
items="["
first=true
while IFS=$'\t' read -r path branch; do
    dir=$(basename "$path")
    if [ "$first" = true ]; then first=false; else items+=","; fi
    items+="{\"value\":\"$path\",\"label\":\"$dir\",\"description\":\"$branch — $path\"}"
done <<< "$worktrees"
items+="]"

echo "{\"type\":\"fuzzy\",\"prompt\":\"Switch to worktree\",\"items\":$items}"

# Read user selection
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

selected=$(echo "$response" | jq -r '.value // empty')
if [ -z "$selected" ]; then
    echo '{"type":"done"}'
    exit 0
fi

echo "{\"type\":\"done\",\"notify\":\"Switched to $selected\"}"
