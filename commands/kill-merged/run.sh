#!/usr/bin/env bash
set -euo pipefail

# Kill Merged Branches - find branches merged into main and delete them along
# with any associated worktrees.

# Find merged branches (exclude main/master and the current branch)
merged=$(git branch --merged main 2>/dev/null | grep -v '^\*' | grep -v 'main' | grep -v 'master' | sed 's/^[[:space:]]*//' || true)

if [ -z "$merged" ]; then
    echo '{"type":"info","message":"No merged branches found"}'
    sleep 1
    echo '{"type":"done"}'
    exit 0
fi

# Build multi-select items
items="["
first=true
while IFS= read -r branch; do
    [ -z "$branch" ] && continue
    worktree=$(git worktree list --porcelain | awk -v b="refs/heads/$branch" '/^worktree /{path=$2} /^branch /{if($2==b) print path}')
    desc="branch only"
    if [ -n "$worktree" ]; then
        desc="+ worktree: $worktree"
    fi
    if [ "$first" = true ]; then first=false; else items+=","; fi
    items+="{\"value\":\"$branch\",\"label\":\"$branch\",\"description\":\"$desc\"}"
done <<< "$merged"
items+="]"

echo "{\"type\":\"multi\",\"prompt\":\"Select merged branches to delete\",\"items\":$items}"

# Read user selection
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

selected=$(echo "$response" | jq -r '.values[]? // empty')
if [ -z "$selected" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Confirm before destructive operation
echo '{"type":"confirm","message":"Delete selected branches and their worktrees?","default":false}'
read -r confirm

# Handle cancellation of confirm
type=$(echo "$confirm" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

confirmed=$(echo "$confirm" | jq -r '.yes')
if [ "$confirmed" != "true" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Perform cleanup
echo '{"type":"info","message":"Cleaning up..."}'
while IFS= read -r branch; do
    [ -z "$branch" ] && continue
    worktree=$(git worktree list --porcelain | awk -v b="refs/heads/$branch" '/^worktree /{path=$2} /^branch /{if($2==b) print path}')
    if [ -n "$worktree" ]; then
        git worktree remove "$worktree" --force 2>/dev/null || true
    fi
    git branch -d "$branch" 2>/dev/null || true
done <<< "$selected"

echo '{"type":"done","notify":"Cleaned up merged branches"}'
